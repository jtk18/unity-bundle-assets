//! Turning textures and sprites into images: find a sprite's placement (atlas or own
//! texture), decode the texture, crop, undo packing rotation, and mask tight-packed sprites to
//! their mesh. Every image comes out top row first.

use crate::bundle::{self, Bundle};
use crate::decode;
use crate::serialized::{class, SerializedFile};
use crate::sprite::{Placement, Rotation, Sprite, SpriteAtlas};
use crate::texture::{is_console_platform, Texture2D};
use crate::{Error, LimitKind, Limits, Result};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// An RGBA8 image, top row first.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Four bytes per pixel, row after row, top row first.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba", &self.rgba.len())
            .finish()
    }
}

impl Image {
    /// An image from its pixels, top row first; `rgba` must hold exactly
    /// `width * height * 4` bytes.
    ///
    /// # Errors
    ///
    /// When the length does not match the dimensions.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let image = Self {
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
                (u64::from(self.width) * u64::from(self.height)).saturating_mul(4)
            )));
        }
        Ok(())
    }

    const fn row(&self) -> usize {
        self.width as usize * 4
    }
}

/// A texture listed by [`Assets::textures`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TextureInfo {
    /// Its path ID, for [`Assets::decode_texture`] and [`Assets::texture`].
    pub path_id: i64,
    /// `m_Name`, or `None` when it could not be read.
    pub name: Option<String>,
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
/// [`Assets::cut`] each sprite; both take `&self`, and `Assets` is `Send + Sync`.
///
/// All the pixels one `Assets` decodes, cuts and masks count towards
/// [`Limits::max_total_work`].
pub struct Assets {
    file: SerializedFile,
    streams: Streams,
    /// Atlases by path ID, read up front; one that fails to read is kept as its error, so it
    /// spoils only the sprites packed into it.
    atlases: HashMap<i64, std::result::Result<SpriteAtlas, Arc<Error>>>,
    /// The most recently decoded texture. Sprites sharing an atlas decode it once when
    /// exported in texture order (see [`Assets::sprites`]).
    cache: Option<(i64, Image)>,
    work: AtomicU64,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assets")
            .field("file", &self.file)
            .field("atlases", &self.atlases.len())
            .field("cached_texture", &self.cache.as_ref().map(|(id, _)| id))
            .field("work", &self.work.load(Ordering::Relaxed))
            .finish()
    }
}

/// Where streamed texture data lives.
enum Streams {
    /// `.resS` files in this folder, an absolute path.
    Dir(PathBuf),
    /// `.resS` entries in the same bundle.
    Bundle(Arc<Bundle>),
}

impl Assets {
    /// Open a serialized file, or an asset bundle holding exactly one, with the default
    /// [`Limits`]. For a bundle holding several (scene bundles), use [`Bundle::parse`] and
    /// [`Assets::from_bundle`].
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is over a limit, or is not a Unity file this crate
    /// reads.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, Limits::default())
    }

    /// [`Assets::open`] under `limits`.
    ///
    /// # Errors
    ///
    /// As [`Assets::open`].
    pub fn open_with(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let path = path.as_ref();
        let data = crate::file::read_limited(path, limits.max_file_size)?;
        if !bundle::is_bundle(&data) {
            let file = SerializedFile::parse_with(data, limits)?;
            return Self::from_serialized(file, path.parent().unwrap_or_else(|| Path::new(".")));
        }
        let bundle = Arc::new(Bundle::parse_with(&data, limits)?);
        drop(data);
        let entries: Vec<bundle::Entry> = bundle.serialized_files().cloned().collect();
        let names: Vec<String> = entries.iter().map(|e| format!("{:?}", e.path())).collect();
        match entries.as_slice() {
            [] => Err(Error::Invalid("bundle holds no serialized file".into())),
            [one] => Self::from_bundle(bundle, one.path()),
            _ => Err(Error::Unsupported(format!(
                "bundle holds {} serialized files ({}{}); open each with Assets::from_bundle",
                names.len(),
                names[..names.len().min(5)].join(", "),
                if names.len() > 5 { ", ..." } else { "" }
            ))),
        }
    }

    /// Open the serialized file at `path` inside `bundle` (a directory path, or an
    /// `archive:/` path ending in one).
    ///
    /// # Errors
    ///
    /// When the bundle has no such entry, or it is not a serialized file this crate reads.
    pub fn from_bundle(bundle: Arc<Bundle>, path: &str) -> Result<Self> {
        let entry = bundle
            .entry(path)
            .ok_or_else(|| Error::NotFound(format!("entry {path:?} in this bundle")))?
            .clone();
        let file = SerializedFile::in_bundle(bundle.clone(), &entry)?;
        Ok(Self::new(file, Streams::Bundle(bundle)))
    }

    /// Wrap a parsed file whose streamed textures live in `stream_dir`.
    ///
    /// # Errors
    ///
    /// When `stream_dir` cannot be made absolute.
    pub fn from_serialized(file: SerializedFile, stream_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = stream_dir.as_ref();
        let dir = std::path::absolute(dir).map_err(Error::io(dir))?;
        Ok(Self::new(file, Streams::Dir(dir)))
    }

    fn new(file: SerializedFile, streams: Streams) -> Self {
        let atlases = file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::SPRITE_ATLAS)
            .map(|o| (o.path_id(), SpriteAtlas::read(&file, o).map_err(Arc::new)))
            .collect();
        Self {
            file,
            streams,
            atlases,
            cache: None,
            work: AtomicU64::new(0),
        }
    }

    /// The serialized file, for reading objects directly.
    #[must_use]
    pub const fn file(&self) -> &SerializedFile {
        &self.file
    }

    /// Charge `pixels` of work against [`Limits::max_total_work`].
    fn charge(&self, pixels: u64) -> Result<()> {
        let limit = self.file.limits().max_total_work;
        let before = self.work.fetch_add(pixels, Ordering::Relaxed);
        Error::limit(LimitKind::TotalWork, before.saturating_add(pixels), limit)
    }

    /// Every sprite whose name passes `keep`, ordered by the texture holding its pixels so
    /// that [`Assets::export`] decodes each texture once. A sprite that cannot be read is
    /// listed in [`SpriteList::skipped`] rather than failing the rest.
    pub fn sprites(&self, mut keep: impl FnMut(&str) -> bool) -> SpriteList {
        let limit = self.file.limits().max_total_triangles;
        let mut list = SpriteList::default();
        let mut triangles = 0u64;
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
            let error = match name {
                None => Error::Invalid("name unreadable".into()),
                Some(_) => {
                    match Sprite::read_within(&self.file, o, limit.saturating_sub(triangles)) {
                        Ok(sprite) => {
                            triangles += sprite.triangles.as_ref().map_or(0, Vec::len) as u64;
                            list.sprites.push(sprite);
                            continue;
                        }
                        Err(e) => e,
                    }
                }
            };
            list.skipped.push(SkippedSprite {
                path_id: o.path_id(),
                name,
                error,
            });
        }
        list.sprites
            .sort_by_cached_key(|s| self.placement(s).ok().map(|p| p.texture.path_id));
        list
    }

    /// Every texture whose name passes `keep` (given `""` when the name cannot be read), in
    /// file order.
    pub fn textures(&self, mut keep: impl FnMut(&str) -> bool) -> Vec<TextureInfo> {
        self.file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::TEXTURE_2D)
            .map(|o| TextureInfo {
                path_id: o.path_id(),
                name: self.file.name(o),
            })
            .filter(|t| keep(t.name.as_deref().unwrap_or("")))
            .collect()
    }

    /// A texture's metadata (size, format, where its pixels live) without decoding or
    /// copying its pixels.
    ///
    /// # Errors
    ///
    /// When there is no such object, it is not a texture, or it cannot be read.
    pub fn texture(&self, path_id: i64) -> Result<Texture2D<'_>> {
        let object = self
            .file
            .object(path_id)
            .ok_or_else(|| Error::NotFound(format!("object {path_id}")))?;
        if object.class_id() != class::TEXTURE_2D {
            return Err(Error::WrongClass {
                path_id,
                found: object.class_id(),
                expected: class::TEXTURE_2D,
            });
        }
        Texture2D::read(&self.file, object)
    }

    /// Where the sprite's pixels are: its atlas entry when it has one, its own render data
    /// otherwise.
    ///
    /// # Errors
    ///
    /// When the sprite's atlas is missing, unreadable, in another file, or does not hold it.
    pub fn placement(&self, sprite: &Sprite) -> Result<Placement> {
        if sprite.atlas.is_null() {
            return Ok(sprite.own);
        }
        let own_texture = !sprite.own.texture.is_null();
        if sprite.atlas.file_id != 0 {
            return if own_texture {
                Ok(sprite.own)
            } else {
                Err(Error::Unsupported(format!(
                    "sprite {:?} is packed into an atlas in another file",
                    sprite.name
                )))
            };
        }
        let id = sprite.atlas.path_id;
        match self.atlases.get(&id) {
            Some(Ok(atlas)) => match atlas.placement(&sprite.render_data_key) {
                Some(p) => Ok(p),
                None if own_texture => Ok(sprite.own),
                None => Err(Error::Invalid(format!(
                    "sprite {:?} is not in its atlas {id}",
                    sprite.name
                ))),
            },
            Some(Err(e)) => Err(Error::AtlasUnreadable {
                atlas: id,
                source: e.clone(),
            }),
            None if own_texture => Ok(sprite.own),
            None => Err(Error::NotFound(format!(
                "atlas {id} of sprite {:?}",
                sprite.name
            ))),
        }
    }

    /// Decode a texture in this file to RGBA8, top row first. Takes `&self`, so textures can
    /// be decoded on several threads at once; pair it with [`Assets::cut`].
    ///
    /// # Errors
    ///
    /// When the texture cannot be read or decoded, or a limit is reached.
    pub fn decode_texture(&self, path_id: i64) -> Result<Image> {
        let texture = self.texture(path_id)?;
        if texture.width == 0 || texture.height == 0 {
            return Err(Error::EmptyTexture(texture.name));
        }
        if is_console_platform(self.file.target_platform()) {
            return Err(Error::Unsupported(format!(
                "texture {:?} was built for a console (platform {}), whose GPU tiling this \
                 crate does not undo",
                texture.name,
                self.file.target_platform()
            )));
        }
        let pixels = u64::from(texture.width) * u64::from(texture.height);
        let limit = self.file.limits().max_texture_pixels;
        Error::limit(LimitKind::TexturePixels, pixels, limit)?;
        if !decode::is_supported(texture.format) {
            return Err(Error::UnsupportedTextureFormat {
                texture: Some(texture.name),
                format: texture.format,
            });
        }
        self.charge(pixels)?;
        let data = match &self.streams {
            Streams::Dir(dir) => texture.data(dir)?,
            Streams::Bundle(bundle) => texture.data_in(bundle)?,
        };
        let rgba =
            decode::decode(texture.format, texture.width, texture.height, &data).map_err(|e| {
                match e {
                    Error::Invalid(what) => {
                        Error::Invalid(format!("texture {:?}: {what}", texture.name))
                    }
                    e => e,
                }
            })?;
        Ok(Image {
            width: texture.width,
            height: texture.height,
            rgba,
        })
    }

    /// Export one sprite, decoding its texture unless it's the one decoded last. The decoded
    /// texture stays cached until the next texture or [`Assets::clear_cache`].
    ///
    /// # Errors
    ///
    /// As [`Assets::decode_texture`] and [`Assets::cut`].
    pub fn export(&mut self, sprite: &Sprite) -> Result<Image> {
        let placement = self.placement(sprite)?;
        let texture = texture_id_of(sprite, &placement)?;
        if self.cache.as_ref().is_none_or(|(id, _)| *id != texture) {
            self.cache = None;
            let image = self.decode_texture(texture)?;
            self.cache = Some((texture, image));
        }
        match &self.cache {
            Some((_, image)) => self.cut_at(sprite, &placement, image),
            None => Err(Error::NotFound(format!("texture {texture}"))),
        }
    }

    /// Drop the texture [`Assets::export`] keeps cached.
    pub fn clear_cache(&mut self) {
        self.cache = None;
    }

    /// The path ID of the texture holding the sprite's pixels.
    ///
    /// # Errors
    ///
    /// As [`Assets::placement`], or when the placement names no texture or one in another
    /// file.
    pub fn texture_id(&self, sprite: &Sprite) -> Result<i64> {
        texture_id_of(sprite, &self.placement(sprite)?)
    }

    /// Cut a sprite out of its decoded texture (from [`Assets::decode_texture`]), top row
    /// first.
    ///
    /// # Errors
    ///
    /// When the image is inconsistent or does not contain the sprite's rect, the sprite uses
    /// packing this crate does not undo, its mask cannot be built, or a limit is reached.
    pub fn cut(&self, sprite: &Sprite, texture: &Image) -> Result<Image> {
        self.cut_at(sprite, &self.placement(sprite)?, texture)
    }

    fn cut_at(&self, sprite: &Sprite, placement: &Placement, texture: &Image) -> Result<Image> {
        texture.check()?;
        if !placement.alpha_texture.is_null() {
            return Err(Error::Unsupported(format!(
                "sprite {:?} keeps its alpha in a separate texture",
                sprite.name
            )));
        }
        if !(placement.downscale.is_finite() && (placement.downscale - 1.0).abs() <= 1e-4) {
            return Err(Error::Unsupported(format!(
                "sprite {:?} is in an atlas scaled by {}",
                sprite.name, placement.downscale
            )));
        }
        let tight = placement.settings.tight();
        if tight && sprite.triangles.is_none() {
            return Err(Error::Unsupported(format!(
                "sprite {:?} is tight-packed, and its mesh is not one this crate reads",
                sprite.name
            )));
        }

        // Rects carry float noise (396.00003): snap values within 1/1000 of a whole pixel,
        // then round outwards. The rect must then lie inside the texture and cover pixels.
        let snap = |v: f32| {
            if (v - v.round()).abs() < 1e-3 {
                v.round()
            } else {
                v
            }
        };
        let r = placement.texture_rect;
        let (x0, y0) = (snap(r.x).floor(), snap(r.y).floor());
        let (x1, y1) = (snap(r.x + r.width).ceil(), snap(r.y + r.height).ceil());
        let (tw, th) = (texture.width as f32, texture.height as f32);
        let inside = [x0, y0, x1, y1].iter().all(|v| v.is_finite())
            && x0 >= 0.0
            && y0 >= 0.0
            && x1 <= tw
            && y1 <= th
            && x1 > x0
            && y1 > y0;
        if !inside {
            return Err(Error::Invalid(format!(
                "sprite {:?} covers {}x{} at {},{}, not inside the {}x{} texture it was given",
                sprite.name, r.width, r.height, r.x, r.y, texture.width, texture.height
            )));
        }
        let (x0, y0, x1, y1) = (x0 as u32, y0 as u32, x1 as u32, y1 as u32);
        self.charge(u64::from(x1 - x0) * u64::from(y1 - y0))?;
        // The texture is top-down and the rect counts from the bottom: take its rows bottom
        // first, as Unity stores them, so rotation and the mesh apply unchanged.
        let mut image = crop_bottom_up(texture, x0, y0, x1 - x0, y1 - y0);

        if placement.settings.packed() {
            image = match placement.settings.rotation() {
                Rotation::Unrotated => image,
                Rotation::FlipHorizontal => flip_horizontal(&image),
                Rotation::FlipVertical => flip_vertical(&image),
                Rotation::Rotate180 => flip_vertical(&flip_horizontal(&image)),
                Rotation::Rotate90 => rotate_90_counterclockwise(&image),
            };
        }
        if let (true, Some(triangles)) = (tight, &sprite.triangles) {
            if !triangles.is_empty() {
                mask(self, &mut image, sprite, triangles, placement)?;
            }
        }
        Ok(flip_vertical(&image))
    }
}

fn texture_id_of(sprite: &Sprite, placement: &Placement) -> Result<i64> {
    if placement.texture.is_null() {
        return Err(Error::Invalid(format!(
            "sprite {:?} has no texture",
            sprite.name
        )));
    }
    if placement.texture.file_id != 0 {
        return Err(Error::Unsupported(format!(
            "sprite {:?} uses a texture in another file",
            sprite.name
        )));
    }
    Ok(placement.texture.path_id)
}

/// The rect `x, y, w, h` (y from the bottom) of a top-down image, rows bottom first.
fn crop_bottom_up(src: &Image, x: u32, y: u32, w: u32, h: u32) -> Image {
    let row = w as usize * 4;
    let mut rgba = Vec::with_capacity(row * h as usize);
    for k in 0..h {
        let line = (src.height - 1 - (y + k)) as usize;
        let start = line * src.row() + x as usize * 4;
        rgba.extend_from_slice(&src.rgba[start..start + row]);
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

fn flip_vertical(src: &Image) -> Image {
    let mut rgba = Vec::with_capacity(src.rgba.len());
    for line in src.rgba.chunks_exact(src.row().max(1)).rev() {
        rgba.extend_from_slice(line);
    }
    Image { rgba, ..*src }
}

fn flip_horizontal(src: &Image) -> Image {
    let mut rgba = Vec::with_capacity(src.rgba.len());
    for line in src.rgba.chunks_exact(src.row().max(1)) {
        for px in line.chunks_exact(4).rev() {
            rgba.extend_from_slice(px);
        }
    }
    Image { rgba, ..*src }
}

/// Undo `Rotate90` packing: a quarter turn counter-clockwise on the stored (bottom-up) rows,
/// as `AssetStudio` does with `Rotate(270)`. `UnityPy` turns the other way; no real sample settles
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
/// `AssetStudio` does). Mesh vertices are in sprite units around the pivot; this maps them into
/// the cropped, bottom-up image. Each triangle is tested only over its own bounding box, and
/// the work is held to [`Limits::max_mask_work`] and charged to the total.
fn mask(
    assets: &Assets,
    image: &mut Image,
    sprite: &Sprite,
    triangles: &[[[f32; 2]; 3]],
    placement: &Placement,
) -> Result<()> {
    let budget = assets.file.limits().max_mask_work;
    let (w, h) = (image.width as usize, image.height as usize);
    let scale = sprite.pixels_to_units;
    let dx = sprite.rect.width * sprite.pivot[0] - placement.texture_rect_offset[0];
    let dy = sprite.rect.height * sprite.pivot[1] - placement.texture_rect_offset[1];
    let mut covered = vec![false; w * h];
    let mut work = 0u64;
    let mut with_area = 0usize;
    for t in triangles {
        let [a, b, c] = t.map(|[x, y]| [x * scale + dx, y * scale + dy]);
        let pts = [a, b, c];
        if pts.iter().flatten().any(|v| !v.is_finite()) {
            return Err(Error::Invalid(format!(
                "sprite {:?} has a mesh vertex that is not a finite number",
                sprite.name
            )));
        }
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        with_area += 1;
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
        Error::limit(LimitKind::MaskWork, work, budget)?;
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
    if with_area == 0 {
        return Err(Error::Invalid(format!(
            "sprite {:?} is tight-packed, and no triangle of its mesh has any area",
            sprite.name
        )));
    }
    assets.charge(work)?;
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

    fn reds(img: &Image) -> Vec<u8> {
        img.rgba.chunks(4).map(|p| p[0]).collect()
    }

    #[test]
    fn test_crop_and_flips() {
        let img = numbered(3, 2); // top-down rows: [0 1 2] [3 4 5]
                                  // Columns 1-2 of both rows, bottom row first.
        assert_eq!(reds(&crop_bottom_up(&img, 1, 0, 2, 2)), [4, 5, 1, 2]);
        // The top row alone (y = 1 counting from the bottom).
        assert_eq!(reds(&crop_bottom_up(&img, 0, 1, 3, 1)), [0, 1, 2]);
        assert_eq!(reds(&flip_vertical(&img)), [3, 4, 5, 0, 1, 2]);
        assert_eq!(reds(&flip_horizontal(&img)), [2, 1, 0, 5, 4, 3]);
    }

    #[test]
    fn test_rotate_90() {
        let img = numbered(3, 2); // rows: [0 1 2] [3 4 5]
                                  // [0 1 2]      [2 5]
                                  // [3 4 5]  ->  [1 4]
                                  //              [0 3]
        let r = rotate_90_counterclockwise(&img);
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(reds(&r), [2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn test_image_new_checks_length() {
        assert!(Image::new(2, 2, vec![0; 16]).is_ok());
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
        assert!(Image::new(u32::MAX, u32::MAX, vec![]).is_err());
    }
}
