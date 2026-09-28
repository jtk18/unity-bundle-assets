//! Turning textures and sprites into images: find a sprite's placement (atlas or own
//! texture), decode the texture, crop, undo packing rotation, and mask tight-packed sprites to
//! their mesh. Every image comes out top row first.

use crate::bundle::{self, Bundle};
use crate::decode;
use crate::serialized::{class, SerializedFile};
use crate::sprite::{Placement, Rotation, Sprite, SpriteAtlas};
use crate::texture::Texture2D;
use crate::{Error, Limits, Result};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An RGBA8 image, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Four bytes per pixel, row after row, top row first.
    pub rgba: Vec<u8>,
}

impl Image {
    /// An image from its pixels; `rgba` must hold exactly `width * height * 4` bytes.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Image> {
        let image = Image {
            width,
            height,
            rgba,
        };
        image.check()?;
        Ok(image)
    }

    fn check(&self) -> Result<()> {
        let want = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|p| p.checked_mul(4));
        if want != Some(self.rgba.len()) {
            return Err(Error::Invalid(format!(
                "a {}x{} image holds {} bytes, not {}",
                self.width,
                self.height,
                self.rgba.len(),
                (self.width as u64 * self.height as u64).saturating_mul(4)
            )));
        }
        Ok(())
    }
}

/// A texture listed by [`Assets::textures`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextureInfo {
    /// Its path ID, for [`Assets::decode_texture`] and [`Assets::texture`].
    pub path_id: i64,
    /// `m_Name`.
    pub name: String,
}

/// What [`Assets::sprites`] found: the sprites it read, and the ones it could not.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct SpriteList {
    /// Sprites read, ordered by the texture holding their pixels.
    pub sprites: Vec<Sprite>,
    /// Sprites that passed the filter (or whose name could not be read) but not the reader.
    pub skipped: Vec<SkippedSprite>,
}

/// A sprite [`Assets::sprites`] could not read.
#[derive(Debug)]
#[non_exhaustive]
pub struct SkippedSprite {
    /// The object's path ID.
    pub path_id: i64,
    /// Its name, when that much could be read.
    pub name: Option<String>,
    /// Why it was skipped.
    pub error: Error,
}

/// One serialized file opened for sprite and texture export: a plain file (`*.assets`,
/// `level*`) or one inside an asset bundle.
///
/// Single-threaded: call [`Assets::export`] on sprites from [`Assets::sprites`]. Parallel:
/// group sprites by [`Assets::texture_id`], then per group [`Assets::decode_texture`] once and
/// [`Assets::cut`] each sprite; both take `&self`.
pub struct Assets {
    file: SerializedFile,
    streams: Streams,
    /// Atlases by path ID, read up front; one that fails to read is kept as its error, so
    /// it spoils only the sprites packed into it.
    atlases: HashMap<i64, std::result::Result<SpriteAtlas, String>>,
    /// The most recently decoded texture. Sprites sharing an atlas decode it once when
    /// exported in texture order (see [`Assets::sprites`]).
    cache: Option<(i64, Image)>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assets")
            .field("file", &self.file)
            .field("atlases", &self.atlases.len())
            .field("cached_texture", &self.cache.as_ref().map(|(id, _)| id))
            .finish()
    }
}

/// Where streamed texture data lives.
enum Streams {
    /// `.resS` files beside the serialized file.
    Dir(PathBuf),
    /// `.resS` entries in the same bundle.
    Bundle(Arc<Bundle>),
}

/// Read a whole file, refusing anything that is not a regular file or is over the limit.
pub(crate) fn read_file(path: &Path, limits: &Limits) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(Error::Unsupported(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > limits.max_file_size {
        return Err(Error::LimitExceeded {
            what: "file size",
            value: meta.len(),
            limit: limits.max_file_size,
        });
    }
    Ok(std::fs::read(path)?)
}

impl Assets {
    /// Open a serialized file, or an asset bundle holding exactly one, with the default
    /// [`Limits`]. For a bundle holding several (scene bundles), use [`Bundle::parse`] and
    /// [`Assets::from_bundle`].
    pub fn open(path: impl AsRef<Path>) -> Result<Assets> {
        Assets::open_with(path, Limits::default())
    }

    /// [`Assets::open`] under `limits`.
    pub fn open_with(path: impl AsRef<Path>, limits: Limits) -> Result<Assets> {
        let path = path.as_ref();
        let data = read_file(path, &limits)?;
        if !bundle::is_bundle(&data) {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            return Assets::new(SerializedFile::parse_with(data, limits)?, Streams::Dir(dir));
        }
        let bundle = Arc::new(Bundle::parse_with(&data, limits)?);
        drop(data);
        let names: Vec<String> = bundle
            .serialized_files()
            .map(|e| e.path().to_string())
            .collect();
        match names.as_slice() {
            [] => Err(Error::Invalid("bundle holds no serialized file".into())),
            [one] => Assets::from_bundle(bundle, one),
            _ => Err(Error::Unsupported(format!(
                "bundle holds {} serialized files ({}); open each with Assets::from_bundle",
                names.len(),
                names.join(", ")
            ))),
        }
    }

    /// Open the serialized file at `path` inside `bundle` (a directory path, or an
    /// `archive:/` path ending in one).
    pub fn from_bundle(bundle: Arc<Bundle>, path: &str) -> Result<Assets> {
        let entry = bundle
            .entry(path)
            .ok_or_else(|| Error::NotFound(format!("entry {path} in this bundle")))?
            .clone();
        let file = SerializedFile::in_bundle(bundle.clone(), &entry)?;
        Assets::new(file, Streams::Bundle(bundle))
    }

    fn new(file: SerializedFile, streams: Streams) -> Result<Assets> {
        let mut atlases = HashMap::new();
        for o in file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::SPRITE_ATLAS)
        {
            atlases
                .entry(o.path_id())
                .or_insert_with(|| SpriteAtlas::read(&file, o).map_err(|e| e.to_string()));
        }
        Ok(Assets {
            file,
            streams,
            atlases,
            cache: None,
        })
    }

    /// The serialized file, for reading objects directly.
    pub fn file(&self) -> &SerializedFile {
        &self.file
    }

    /// Every sprite whose name passes `keep`, ordered by the texture holding its pixels so
    /// that [`Assets::export`] decodes each texture once. A sprite that cannot be read is
    /// listed in [`SpriteList::skipped`] rather than failing the rest.
    pub fn sprites(&self, keep: impl Fn(&str) -> bool) -> SpriteList {
        let limits = self.file.limits();
        let mut list = SpriteList::default();
        let mut triangles = 0usize;
        for o in self
            .file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::SPRITE)
        {
            let name = self.file.name(o);
            if name.as_deref().is_some_and(|n| !keep(n)) {
                continue;
            }
            let skip = |error| SkippedSprite {
                path_id: o.path_id(),
                name: name.clone(),
                error,
            };
            if name.is_none() {
                list.skipped
                    .push(skip(Error::Invalid("name unreadable".into())));
                continue;
            }
            match Sprite::read(&self.file, o) {
                Ok(sprite) => {
                    triangles = triangles.saturating_add(sprite.triangles.len());
                    if triangles > limits.max_total_triangles {
                        list.skipped.push(skip(Error::LimitExceeded {
                            what: "sprite mesh triangles in this file",
                            value: triangles as u64,
                            limit: limits.max_total_triangles as u64,
                        }));
                        triangles -= sprite.triangles.len();
                    } else {
                        list.sprites.push(sprite);
                    }
                }
                Err(e) => list.skipped.push(skip(e)),
            }
        }
        list.sprites
            .sort_by_cached_key(|s| self.placement(s).ok().map(|p| p.texture.path_id));
        list
    }

    /// Every texture whose name passes `keep`, in file order. Objects whose name cannot be
    /// read are left out.
    pub fn textures(&self, keep: impl Fn(&str) -> bool) -> Vec<TextureInfo> {
        self.file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::TEXTURE_2D)
            .filter_map(|o| {
                Some(TextureInfo {
                    path_id: o.path_id(),
                    name: self.file.name(o)?,
                })
            })
            .filter(|t| keep(&t.name))
            .collect()
    }

    /// A texture's metadata (size, format, where its pixels live) without decoding it.
    pub fn texture(&self, path_id: i64) -> Result<Texture2D> {
        let object = self
            .file
            .object(path_id)
            .filter(|o| o.class_id() == class::TEXTURE_2D)
            .ok_or_else(|| Error::NotFound(format!("Texture2D {path_id}")))?;
        Texture2D::read(&self.file, object)
    }

    /// Where the sprite's pixels are: its atlas entry when it has one, its own render data
    /// otherwise.
    pub fn placement(&self, sprite: &Sprite) -> Result<Placement> {
        if sprite.atlas.is_null() {
            return Ok(sprite.own);
        }
        if sprite.atlas.file_id != 0 {
            return if sprite.own.texture.is_null() {
                Err(Error::Unsupported(format!(
                    "sprite {} is packed into an atlas in another file",
                    sprite.name
                )))
            } else {
                Ok(sprite.own)
            };
        }
        let id = sprite.atlas.path_id;
        match self.atlases.get(&id) {
            Some(Ok(atlas)) => match atlas.placement(&sprite.render_data_key) {
                Some(p) => Ok(p),
                None if !sprite.own.texture.is_null() => Ok(sprite.own),
                None => Err(Error::Invalid(format!(
                    "sprite {} is not in its atlas {id}",
                    sprite.name
                ))),
            },
            Some(Err(e)) => Err(Error::Invalid(format!(
                "sprite {}'s atlas {id} could not be read: {e}",
                sprite.name
            ))),
            None if !sprite.own.texture.is_null() => Ok(sprite.own),
            None => Err(Error::NotFound(format!(
                "atlas {id} of sprite {}",
                sprite.name
            ))),
        }
    }

    /// Decode a texture in this file to RGBA8, top row first. Takes `&self`, so textures can
    /// be decoded on several threads at once; pair it with [`Assets::cut`].
    pub fn decode_texture(&self, path_id: i64) -> Result<Image> {
        let texture = self.texture(path_id)?;
        if texture.width == 0 || texture.height == 0 {
            return Err(Error::EmptyTexture(texture.name));
        }
        let limit = self.file.limits().max_texture_pixels;
        let pixels = texture.width as u64 * texture.height as u64;
        if pixels > limit {
            return Err(Error::LimitExceeded {
                what: "texture pixels",
                value: pixels,
                limit,
            });
        }
        if texture.is_switch_swizzled(self.file.target_platform()) {
            return Err(Error::Unsupported(format!(
                "texture {} is swizzled for Nintendo Switch",
                texture.name
            )));
        }
        let data = match &self.streams {
            Streams::Dir(dir) => texture.data(dir)?,
            Streams::Bundle(bundle) => texture.data_in(bundle)?,
        };
        let rgba =
            decode::decode(texture.format, texture.width, texture.height, &data).map_err(|e| {
                match e {
                    Error::UnsupportedTextureFormat { format, .. } => {
                        Error::UnsupportedTextureFormat {
                            texture: texture.name.clone(),
                            format,
                        }
                    }
                    Error::Invalid(what) => {
                        Error::Invalid(format!("texture {}: {what}", texture.name))
                    }
                    e => e,
                }
            })?;
        Ok(flip_vertical(&Image {
            width: texture.width,
            height: texture.height,
            rgba,
        }))
    }

    /// Export one sprite, decoding its texture unless it's the one decoded last. The decoded
    /// texture stays cached until the next texture or [`Assets::clear_cache`].
    pub fn export(&mut self, sprite: &Sprite) -> Result<Image> {
        let texture = self.texture_id(sprite)?;
        if self.cache.as_ref().is_none_or(|(id, _)| *id != texture) {
            self.cache = None;
            self.cache = Some((texture, self.decode_texture(texture)?));
        }
        let (_, image) = self.cache.as_ref().expect("filled above");
        self.cut(sprite, image)
    }

    /// Drop the texture [`Assets::export`] keeps cached.
    pub fn clear_cache(&mut self) {
        self.cache = None;
    }

    /// The path ID of the texture holding the sprite's pixels.
    pub fn texture_id(&self, sprite: &Sprite) -> Result<i64> {
        let placement = self.placement(sprite)?;
        if placement.texture.is_null() {
            return Err(Error::Invalid(format!(
                "sprite {} has no texture",
                sprite.name
            )));
        }
        if placement.texture.file_id != 0 {
            return Err(Error::Unsupported(format!(
                "sprite {} uses a texture in another file",
                sprite.name
            )));
        }
        Ok(placement.texture.path_id)
    }

    /// Cut a sprite out of its decoded texture (from [`Assets::decode_texture`]), top row
    /// first.
    pub fn cut(&self, sprite: &Sprite, texture: &Image) -> Result<Image> {
        texture.check()?;
        let placement = self.placement(sprite)?;
        if !placement.alpha_texture.is_null() {
            return Err(Error::Unsupported(format!(
                "sprite {} keeps its alpha in a separate texture",
                sprite.name
            )));
        }
        if placement.downscale.is_finite() && (placement.downscale - 1.0).abs() > 1e-4 {
            return Err(Error::Unsupported(format!(
                "sprite {} is in an atlas downscaled by {}",
                sprite.name, placement.downscale
            )));
        }
        let tight = placement.settings.tight();
        if tight && sprite.mesh_unreadable {
            return Err(Error::Unsupported(format!(
                "sprite {} is tight-packed, and its mesh layout is not one this crate reads",
                sprite.name
            )));
        }

        // Rects carry float noise (396.00003); snap values that are within 1/1000 of a whole
        // pixel before rounding outwards.
        let snap = |v: f32| {
            if (v - v.round()).abs() < 1e-3 {
                v.round()
            } else {
                v
            }
        };
        let r = placement.texture_rect;
        let (tw, th) = (texture.width as f32, texture.height as f32);
        let (fx0, fy0) = (snap(r.x).floor(), snap(r.y).floor());
        let (fx1, fy1) = (snap(r.x + r.width).ceil(), snap(r.y + r.height).ceil());
        let finite = [fx0, fy0, fx1, fy1].iter().all(|v| v.is_finite());
        if !finite || fx0 < -1.0 || fy0 < -1.0 || fx1 > tw + 1.0 || fy1 > th + 1.0 {
            return Err(Error::Invalid(format!(
                "sprite {} lies at {}x{}+{}+{}, outside the {}x{} texture it was given",
                sprite.name, r.width, r.height, r.x, r.y, texture.width, texture.height
            )));
        }
        let x0 = (fx0.max(0.0) as u32).min(texture.width);
        let y0 = (fy0.max(0.0) as u32).min(texture.height);
        let x1 = (fx1.max(0.0) as u32).clamp(x0, texture.width);
        let y1 = (fy1.max(0.0) as u32).clamp(y0, texture.height);
        // The texture is top-down; the sprite's rect counts from the bottom. Crop, then work
        // bottom-up as Unity stores it.
        let top = texture.height - y1;
        let mut image = flip_vertical(&crop(texture, x0, top, x1 - x0, y1 - y0));

        if placement.settings.packed() {
            image = match placement.settings.rotation() {
                Rotation::None => image,
                Rotation::FlipHorizontal => flip_horizontal(&image),
                Rotation::FlipVertical => flip_vertical(&image),
                Rotation::Rotate180 => flip_vertical(&flip_horizontal(&image)),
                Rotation::Rotate90 => rotate_90_counterclockwise(&image),
            };
        }
        if tight && !sprite.triangles.is_empty() {
            mask(
                &mut image,
                sprite,
                &placement,
                self.file.limits().max_mask_work,
            )?;
        }
        Ok(flip_vertical(&image))
    }
}

fn crop(src: &Image, x: u32, y: u32, w: u32, h: u32) -> Image {
    let row = w as usize * 4;
    let mut rgba = Vec::with_capacity(row * h as usize);
    for line in y..y + h {
        let start = (line as usize * src.width as usize + x as usize) * 4;
        rgba.extend_from_slice(&src.rgba[start..start + row]);
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

fn flip_vertical(src: &Image) -> Image {
    let row = src.width as usize * 4;
    let rgba = src
        .rgba
        .chunks_exact(row.max(1))
        .rev()
        .flatten()
        .copied()
        .collect();
    Image { rgba, ..*src }
}

fn flip_horizontal(src: &Image) -> Image {
    let row = src.width as usize * 4;
    let mut rgba = Vec::with_capacity(src.rgba.len());
    for line in src.rgba.chunks_exact(row.max(1)) {
        rgba.extend(line.chunks_exact(4).rev().flatten());
    }
    Image { rgba, ..*src }
}

/// Undo `Rotate90` packing: a quarter turn counter-clockwise on the stored (bottom-up) rows,
/// as AssetStudio does with `Rotate(270)`. UnityPy turns the other way; no real sample settles
/// which is right.
fn rotate_90_counterclockwise(src: &Image) -> Image {
    let (w, h) = (src.width as usize, src.height as usize);
    let mut rgba = vec![0; src.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let from = (y * w + x) * 4;
            let (nx, ny) = (y, w - 1 - x);
            let to = (ny * h + nx) * 4;
            rgba[to..to + 4].copy_from_slice(&src.rgba[from..from + 4]);
        }
    }
    Image {
        width: src.height,
        height: src.width,
        rgba,
    }
}

/// Clear every pixel whose centre lies outside the sprite's mesh (colour and alpha both, as
/// AssetStudio does). Mesh vertices are in sprite units around the pivot; this maps them into
/// the cropped, bottom-up image. Each triangle is tested only over its own bounding box, and
/// the total work is held to `budget`.
fn mask(image: &mut Image, sprite: &Sprite, placement: &Placement, budget: u64) -> Result<()> {
    let (w, h) = (image.width as usize, image.height as usize);
    let scale = sprite.pixels_to_units;
    let dx = sprite.rect.width * sprite.pivot[0] - placement.texture_rect_offset[0];
    let dy = sprite.rect.height * sprite.pivot[1] - placement.texture_rect_offset[1];
    let mut covered = vec![false; w * h];
    let mut work = 0u64;
    for t in &sprite.triangles {
        let [a, b, c] = t.map(|[x, y]| [x * scale + dx, y * scale + dy]);
        let pts = [a, b, c];
        if pts.iter().flatten().any(|v| !v.is_finite()) {
            continue;
        }
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        let min = |i: usize| pts.iter().map(|p| p[i]).fold(f32::INFINITY, f32::min);
        let max = |i: usize| pts.iter().map(|p| p[i]).fold(f32::NEG_INFINITY, f32::max);
        // Pixel centres at +0.5: pixel p is a candidate when min <= p + 0.5 <= max.
        let x0 = ((min(0) - 0.5).ceil().max(0.0) as usize).min(w);
        let x1 = (((max(0) - 0.5).floor() + 1.0).max(0.0) as usize).min(w);
        let y0 = ((min(1) - 0.5).ceil().max(0.0) as usize).min(h);
        let y1 = (((max(1) - 0.5).floor() + 1.0).max(0.0) as usize).min(h);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        work = work.saturating_add(((x1 - x0) * (y1 - y0)) as u64);
        if work > budget {
            return Err(Error::LimitExceeded {
                what: "sprite mask work",
                value: work,
                limit: budget,
            });
        }
        let side = |p: [f32; 2], q: [f32; 2], px: f32, py: f32| {
            (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0])
        };
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let (d1, d2, d3) = (side(a, b, px, py), side(b, c, px, py), side(c, a, px, py));
                let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                if !(neg && pos) {
                    covered[y * w + x] = true;
                }
            }
        }
    }
    for (px, keep) in image.rgba.chunks_exact_mut(4).zip(&covered) {
        if !keep {
            px.fill(0);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(w: u32, h: u32) -> Image {
        Image {
            width: w,
            height: h,
            rgba: (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect(),
        }
    }

    #[test]
    fn test_crop_and_flips() {
        let img = numbered(3, 2); // rows: [0 1 2] [3 4 5]
        let c = crop(&img, 1, 0, 2, 2);
        assert_eq!(
            c.rgba.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [1, 2, 4, 5]
        );
        let v = flip_vertical(&img);
        assert_eq!(
            v.rgba.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [3, 4, 5, 0, 1, 2]
        );
        let h = flip_horizontal(&img);
        assert_eq!(
            h.rgba.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [2, 1, 0, 5, 4, 3]
        );
    }

    #[test]
    fn test_rotate_90() {
        let img = numbered(3, 2); // rows: [0 1 2] [3 4 5]
                                  // [0 1 2]      [2 5]
                                  // [3 4 5]  ->  [1 4]
                                  //              [0 3]
        let r = rotate_90_counterclockwise(&img);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(
            r.rgba.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [2, 5, 1, 4, 0, 3]
        );
    }

    #[test]
    fn test_image_new_checks_length() {
        assert!(Image::new(2, 2, vec![0; 16]).is_ok());
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
        assert!(Image::new(u32::MAX, u32::MAX, vec![]).is_err());
    }
}
