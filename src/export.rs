//! Turning a sprite into an image: find its placement (atlas or own texture), decode the
//! texture, crop, undo packing rotation, mask tight-packed sprites to their mesh, and flip to
//! top-down rows.

use crate::bundle::{self, Bundle};
use crate::decode;
use crate::serialized::{class, SerializedFile};
use crate::sprite::{Placement, Rotation, Sprite, SpriteAtlas};
use crate::texture::Texture2D;
use crate::{Error, Result};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An RGBA8 image, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// One serialized file opened for sprite and texture export: a plain file (`*.assets`,
/// `level*`) or one inside an asset bundle.
///
/// Single-threaded: call [`Assets::export`] on sprites from [`Assets::sprites`]. Parallel:
/// group sprites by [`Assets::texture_id`], then per group [`Assets::decode_texture`] once and
/// [`Assets::cut`] each sprite; both take `&self`.
pub struct Assets {
    pub file: SerializedFile,
    streams: Streams,
    atlases: HashMap<i64, SpriteAtlas>,
    /// The most recently decoded texture. Sprites sharing an atlas decode it once when
    /// exported in texture order (see [`Assets::sprites`]).
    cache: Option<(i64, Image)>,
}

/// Where streamed texture data lives.
enum Streams {
    /// `.resS` files beside the serialized file.
    Dir(PathBuf),
    /// `.resS` entries in the same bundle.
    Bundle(Arc<Bundle>),
}

impl Assets {
    /// Open a serialized file, or an asset bundle holding exactly one. For a bundle holding
    /// several (scene bundles), use [`Bundle::parse`] and [`Assets::from_bundle`].
    pub fn open(path: &Path) -> Result<Assets> {
        let data = std::fs::read(path)?;
        if !bundle::is_bundle(&data) {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            return Assets::new(SerializedFile::parse(data)?, Streams::Dir(dir));
        }
        let bundle = Arc::new(Bundle::parse(&data)?);
        drop(data);
        let names: Vec<String> = bundle.serialized_files().map(|e| e.path.clone()).collect();
        match names.as_slice() {
            [one] => Assets::from_bundle(bundle.clone(), one),
            _ => Err(Error::Unsupported(format!(
                "bundle holds {} serialized files ({}); open each with Assets::from_bundle",
                names.len(),
                names.join(", ")
            ))),
        }
    }

    /// Open the serialized file at `path` inside `bundle`.
    pub fn from_bundle(bundle: Arc<Bundle>, path: &str) -> Result<Assets> {
        let entry = bundle
            .entries
            .iter()
            .find(|e| e.path == path)
            .ok_or_else(|| Error::Invalid(format!("bundle has no entry {path}")))?;
        let file = SerializedFile::parse(bundle.bytes(entry).to_vec())?;
        Assets::new(file, Streams::Bundle(bundle))
    }

    fn new(file: SerializedFile, streams: Streams) -> Result<Assets> {
        let mut atlases = HashMap::new();
        for o in file
            .objects
            .iter()
            .filter(|o| o.class_id == class::SPRITE_ATLAS)
        {
            atlases.insert(o.path_id, SpriteAtlas::read(&file, o)?);
        }
        Ok(Assets {
            file,
            streams,
            atlases,
            cache: None,
        })
    }

    /// Every sprite whose name passes `keep`, ordered by the texture holding its pixels so
    /// that [`Assets::export`] decodes each texture once.
    pub fn sprites(&self, keep: impl Fn(&str) -> bool) -> Result<Vec<Sprite>> {
        let mut out = Vec::new();
        for o in self
            .file
            .objects
            .iter()
            .filter(|o| o.class_id == class::SPRITE)
        {
            if !self.file.name(o).is_some_and(|n| keep(&n)) {
                continue;
            }
            out.push(Sprite::read(&self.file, o)?);
        }
        out.sort_by_key(|s| self.placement(s).texture.path_id);
        Ok(out)
    }

    /// Every texture whose name passes `keep`, as (path ID, name), for
    /// [`Assets::decode_texture`].
    pub fn textures(&self, keep: impl Fn(&str) -> bool) -> Vec<(i64, String)> {
        self.file
            .objects
            .iter()
            .filter(|o| o.class_id == class::TEXTURE_2D)
            .filter_map(|o| Some((o.path_id, self.file.name(o)?)))
            .filter(|(_, name)| keep(name))
            .collect()
    }

    /// Where the sprite's pixels are: its atlas entry when it has one, its own render
    /// data otherwise.
    pub fn placement(&self, sprite: &Sprite) -> Placement {
        self.atlases
            .get(&sprite.atlas.path_id)
            .filter(|_| sprite.atlas.file_id == 0)
            .and_then(|a| a.placement(&sprite.render_data_key))
            .unwrap_or(sprite.own)
    }

    /// Decode a texture in this file to RGBA8, bottom row first (Unity's order). Takes
    /// `&self`, so textures can be decoded on several threads at once; pair it with
    /// [`Assets::cut`].
    pub fn decode_texture(&self, path_id: i64) -> Result<Image> {
        let object = self
            .file
            .object(path_id)
            .ok_or_else(|| Error::Invalid(format!("no texture object {path_id}")))?;
        if object.class_id != class::TEXTURE_2D {
            return Err(Error::Invalid(format!(
                "object {path_id} is not a Texture2D"
            )));
        }
        let texture = Texture2D::read(&self.file, object)?;
        let data = match &self.streams {
            Streams::Dir(dir) => texture.data(dir)?,
            Streams::Bundle(bundle) => texture.data_in(bundle)?,
        };
        let rgba =
            decode::decode(texture.format, texture.width, texture.height, &data).map_err(|e| {
                match e {
                    Error::Unsupported(what) => {
                        Error::Unsupported(format!("{what} (texture {})", texture.name))
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

    /// Export one sprite, decoding its texture unless it's the one decoded last.
    pub fn export(&mut self, sprite: &Sprite) -> Result<Image> {
        let texture = self.texture_id(sprite)?;
        if self.cache.as_ref().is_none_or(|(id, _)| *id != texture) {
            self.cache = Some((texture, self.decode_texture(texture)?));
        }
        self.cut(sprite, &self.cache.as_ref().unwrap().1)
    }

    /// The path ID of the texture holding the sprite's pixels.
    pub fn texture_id(&self, sprite: &Sprite) -> Result<i64> {
        let placement = self.placement(sprite);
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

    /// Cut a sprite out of its decoded texture (from [`Assets::decode_texture`]).
    pub fn cut(&self, sprite: &Sprite, texture: &Image) -> Result<Image> {
        let placement = self.placement(sprite);
        let r = placement.texture_rect;
        let x0 = (r.x.floor().max(0.0) as u32).min(texture.width);
        let y0 = (r.y.floor().max(0.0) as u32).min(texture.height);
        let x1 = ((r.x + r.width).ceil().max(0.0) as u32).clamp(x0, texture.width);
        let y1 = ((r.y + r.height).ceil().max(0.0) as u32).clamp(y0, texture.height);
        let mut image = crop(texture, x0, y0, x1 - x0, y1 - y0);

        if placement.settings.packed() {
            image = match placement.settings.rotation() {
                Rotation::None => image,
                Rotation::FlipHorizontal => flip_horizontal(&image),
                Rotation::FlipVertical => flip_vertical(&image),
                Rotation::Rotate180 => flip_vertical(&flip_horizontal(&image)),
                Rotation::Rotate90 => rotate_90_counterclockwise(&image),
            };
        }
        if placement.settings.tight() && !sprite.triangles.is_empty() {
            mask(&mut image, sprite, &placement);
        }
        Ok(flip_vertical(&image))
    }
}

fn crop(src: &Image, x: u32, y: u32, w: u32, h: u32) -> Image {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in y..y + h {
        let start = ((row * src.width + x) * 4) as usize;
        rgba.extend_from_slice(&src.rgba[start..start + (w * 4) as usize]);
    }
    Image {
        width: w,
        height: h,
        rgba,
    }
}

fn flip_vertical(src: &Image) -> Image {
    let row = (src.width * 4) as usize;
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
    let row = (src.width * 4) as usize;
    let mut rgba = Vec::with_capacity(src.rgba.len());
    for line in src.rgba.chunks_exact(row.max(1)) {
        rgba.extend(line.chunks_exact(4).rev().flatten());
    }
    Image { rgba, ..*src }
}

/// Undo `Rotate90` packing: a quarter turn counter-clockwise on the stored (bottom-up) rows,
/// as AssetStudio does with `Rotate(270)`.
fn rotate_90_counterclockwise(src: &Image) -> Image {
    let (w, h) = (src.width, src.height);
    let mut rgba = vec![0; src.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let from = ((y * w + x) * 4) as usize;
            let (nx, ny) = (y, w - 1 - x);
            let to = ((ny * h + nx) * 4) as usize;
            rgba[to..to + 4].copy_from_slice(&src.rgba[from..from + 4]);
        }
    }
    Image {
        width: h,
        height: w,
        rgba,
    }
}

/// Clear every pixel whose centre lies outside the sprite's mesh. Mesh vertices are in
/// sprite units around the pivot; this maps them into the cropped image.
fn mask(image: &mut Image, sprite: &Sprite, placement: &Placement) {
    let scale = sprite.pixels_to_units;
    let dx = sprite.rect.width * sprite.pivot[0] - placement.texture_rect_offset[0];
    let dy = sprite.rect.height * sprite.pivot[1] - placement.texture_rect_offset[1];
    let triangles: Vec<[[f32; 2]; 3]> = sprite
        .triangles
        .iter()
        .map(|t| t.map(|[x, y]| [x * scale + dx, y * scale + dy]))
        .collect();
    let inside = |px: f32, py: f32| {
        triangles.iter().any(|[a, b, c]| {
            let side = |p: [f32; 2], q: [f32; 2]| {
                (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0])
            };
            let (d1, d2, d3) = (side(*a, *b), side(*b, *c), side(*c, *a));
            let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            !(neg && pos)
        })
    };
    for y in 0..image.height {
        for x in 0..image.width {
            if !inside(x as f32 + 0.5, y as f32 + 0.5) {
                let at = ((y * image.width + x) * 4) as usize;
                image.rgba[at..at + 4].fill(0);
            }
        }
    }
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
}
