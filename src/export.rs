//! Turning textures and sprites into images: find a sprite's placement (atlas or own
//! texture), decode the texture, then cut the sprite out in one pass: crop, undo packing
//! rotation, and mask tight-packed sprites to their mesh. Every image comes out top row first.

use crate::bundle::{self, Bundle, Entry};
use crate::decode;
use crate::serialized::{class, SerializedFile};
use crate::sprite::{Placement, Rotation, Sprite, SpriteAtlas};
use crate::texture::{is_console_platform, Texture2D};
use crate::{Error, LimitKind, Limits, Reservation, Result, Shared};

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
#[derive(Default)]
#[non_exhaustive]
pub struct SpriteList {
    /// Sprites read, ordered by the texture holding their pixels.
    pub sprites: Vec<Sprite>,
    /// Sprites that passed the filter (or whose name could not be read) but not the reader.
    pub skipped: Vec<SkippedSprite>,
}

impl std::fmt::Debug for SpriteList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpriteList")
            .field("sprites", &self.sprites.len())
            .field("skipped", &self.skipped.len())
            .finish()
    }
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
/// [`Assets::cut`] each sprite; both take `&self`, and `Assets` is `Send + Sync`. Each thread
/// then holds a decoded texture, up to 1 GiB at the default limits.
///
/// Everything opened from one file or bundle shares one [`Limits::max_total_work`].
pub struct Assets {
    file: SerializedFile,
    streams: Streams,
    /// This file, as named in errors about stream ranges.
    owner: String,
    /// Atlases by path ID, read when first needed; one that fails to read is kept as its
    /// error, so it spoils only the sprites packed into it.
    atlases: HashMap<i64, OnceLock<std::result::Result<SpriteAtlas, Arc<Error>>>>,
    /// The most recently decoded texture. Sprites sharing an atlas decode it once when
    /// exported in texture order (see [`Assets::sprites`]).
    cache: Option<(i64, Image)>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assets")
            .field("file", &self.file)
            .field("atlases", &self.atlases.len())
            .field("cached_texture", &self.cache.as_ref().map(|(id, _)| id))
            .field("work_done", &self.shared.work())
            .finish_non_exhaustive()
    }
}

/// Where streamed texture data lives.
enum Streams {
    /// `.resS` files in this folder, an absolute path.
    Dir(PathBuf),
    /// `.resS` entries in the same bundle.
    Bundle(Arc<Bundle>),
}

/// The folder holding `path`: its parent, or `.` for a bare file name.
fn folder_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
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
        Self::from_bytes(data, folder_of(path), limits)
    }

    /// Open a serialized file or single-file bundle from its bytes. A serialized file's
    /// streamed textures are read from `stream_dir`.
    ///
    /// # Errors
    ///
    /// As [`Assets::open`].
    pub fn from_bytes(data: Vec<u8>, stream_dir: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        if !bundle::is_bundle(&data) {
            let file = SerializedFile::parse_with(data, limits)?;
            return Self::from_serialized(file, stream_dir);
        }
        let bundle = Arc::new(Bundle::parse_with(&data, limits)?);
        drop(data);
        let mut files = bundle.serialized_files();
        let first: Option<Entry> = files.next().cloned();
        let more = files.count();
        match first {
            None => Err(Error::Invalid("bundle holds no serialized file".into())),
            Some(one) if more == 0 => Self::from_bundle(bundle, one.path()),
            Some(_) => {
                let names: Vec<String> = bundle
                    .serialized_files()
                    .take(5)
                    .map(|e| format!("{:?}", e.path()))
                    .collect();
                Err(Error::Unsupported(format!(
                    "bundle holds {} serialized files ({}{}); open each with Assets::from_bundle",
                    more + 1,
                    names.join(", "),
                    if more + 1 > 5 { ", ..." } else { "" }
                )))
            }
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
        let shared = bundle.shared();
        Ok(Self::new(
            file,
            Streams::Bundle(bundle),
            entry.path().to_string(),
            shared,
        ))
    }

    /// Wrap a parsed file whose streamed textures live in `stream_dir` (`""` means the
    /// current folder).
    ///
    /// # Errors
    ///
    /// When `stream_dir` cannot be made absolute.
    pub fn from_serialized(file: SerializedFile, stream_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = stream_dir.as_ref();
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let dir = std::path::absolute(dir).map_err(Error::io(dir))?;
        let owner = dir.display().to_string();
        Ok(Self::new(
            file,
            Streams::Dir(dir),
            owner,
            Arc::new(Shared::default()),
        ))
    }

    fn new(file: SerializedFile, streams: Streams, owner: String, shared: Arc<Shared>) -> Self {
        let atlases = file
            .objects()
            .iter()
            .filter(|o| o.class_id() == class::SPRITE_ATLAS)
            .map(|o| (o.path_id(), OnceLock::new()))
            .collect();
        Self {
            file,
            streams,
            owner,
            atlases,
            cache: None,
            shared,
        }
    }

    /// The serialized file, for reading objects directly.
    #[must_use]
    pub const fn file(&self) -> &SerializedFile {
        &self.file
    }

    /// Pixels decoded, cut and masked so far, counted against [`Limits::max_total_work`] and
    /// shared with everything else opened from the same bundle.
    #[must_use]
    pub fn work_done(&self) -> u64 {
        self.shared.work()
    }

    fn reserve(&self, amount: u64) -> Result<Reservation<'_>> {
        self.shared
            .reserve(amount, self.file.limits().max_total_work)
    }

    /// The atlas with this path ID, read on first use.
    fn atlas(&self, id: i64) -> Option<&std::result::Result<SpriteAtlas, Arc<Error>>> {
        self.atlases.get(&id).map(|cell| {
            cell.get_or_init(|| {
                self.file
                    .object(id)
                    .ok_or_else(|| Error::NotFound(format!("atlas {id}")))
                    .and_then(|o| SpriteAtlas::read(&self.file, o))
                    .map_err(Arc::new)
            })
        })
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
        list.sprites.sort_by_cached_key(|s| self.texture_key(s));
        list
    }

    /// The path ID of the texture a sprite probably lives in, for ordering; `None` when that
    /// cannot be told without an error.
    fn texture_key(&self, sprite: &Sprite) -> Option<i64> {
        let own = (!sprite.own.texture.is_null()).then_some(sprite.own.texture.path_id);
        if sprite.atlas.is_null() || sprite.atlas.file_id != 0 {
            return own;
        }
        match self.atlas(sprite.atlas.path_id) {
            Some(Ok(atlas)) => atlas
                .placement(&sprite.render_data_key)
                .map(|p| p.texture.path_id)
                .or(own),
            _ => own,
        }
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
        match self.atlas(id) {
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
                error: e.clone(),
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
        let reservation = self.reserve(pixels)?;
        if let Some(stream) = &texture.stream {
            let owner = format!("texture {path_id} of {:?}", self.owner);
            let end = stream.offset.saturating_add(u64::from(stream.size));
            self.shared
                .claim_stream(&stream.path, stream.offset, end, &owner)?;
        }
        let data = match &self.streams {
            Streams::Dir(dir) => texture.data(dir)?,
            Streams::Bundle(bundle) => texture.data_in(bundle)?,
        };
        let (format, width, height) = (texture.format, texture.width, texture.height);
        let rgba = match data {
            Cow::Owned(v) => decode::decode_owned(format, width, height, v),
            Cow::Borrowed(b) => decode::decode(format, width, height, b),
        }
        .map_err(|e| match e {
            Error::Invalid(what) => Error::Invalid(format!("texture {:?}: {what}", texture.name)),
            e => e,
        })?;
        reservation.keep();
        Ok(Image {
            width,
            height,
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
        let name = &sprite.name;
        if !placement.alpha_texture.is_null() {
            return Err(Error::Unsupported(format!(
                "sprite {name:?} keeps its alpha in a separate texture"
            )));
        }
        if !(placement.downscale.is_finite() && (placement.downscale - 1.0).abs() <= 1e-4) {
            return Err(Error::Unsupported(format!(
                "sprite {name:?} is in an atlas scaled by {}",
                placement.downscale
            )));
        }
        let settings = placement.settings;
        let rotation = if settings.packed() {
            settings.rotation().ok_or_else(|| {
                Error::Unsupported(format!(
                    "sprite {name:?} has packing rotation {}, which Unity does not define",
                    (settings.0 >> 2) & 0xf
                ))
            })?
        } else {
            Rotation::Unrotated
        };
        let triangles = match (settings.tight(), &sprite.triangles) {
            (false, _) => None,
            (true, None) => {
                return Err(Error::Unsupported(format!(
                    "sprite {name:?} is tight-packed, and its mesh is not one this crate reads"
                )))
            }
            (true, Some(t)) if t.is_empty() => {
                return Err(Error::Invalid(format!(
                    "sprite {name:?} is tight-packed, and its mesh is empty"
                )))
            }
            (true, Some(t)) => Some(t.as_slice()),
        };

        // Rects carry float noise (396.00003): snap values within a thousandth of a pixel (or
        // a few units in the last place, for large ones) to the whole pixel, then round
        // outwards. The rect must then lie inside the texture and cover pixels.
        let snap = |v: f32| {
            let tolerance = 1e-3_f32.max(v.abs() * f32::EPSILON * 4.0);
            if (v - v.round()).abs() <= tolerance {
                v.round()
            } else {
                v
            }
        };
        let r = placement.texture_rect;
        let bounds = [
            snap(r.x).floor(),
            snap(r.y).floor(),
            snap(r.x + r.width).ceil(),
            snap(r.y + r.height).ceil(),
        ];
        let [x0, y0, x1, y1] = bounds.map(|v| if v.is_finite() { v as i64 } else { -1 });
        let (tw, th) = (i64::from(texture.width), i64::from(texture.height));
        if !(0 <= x0 && x0 < x1 && x1 <= tw && 0 <= y0 && y0 < y1 && y1 <= th) {
            return Err(Error::Invalid(format!(
                "sprite {name:?} covers {}x{} at {},{}, not inside the {}x{} texture it was given",
                r.width, r.height, r.x, r.y, texture.width, texture.height
            )));
        }
        // All four are within the texture, so they fit u32.
        let (x0, y0) = (x0 as u32, y0 as u32);
        let (w, h) = ((x1 - i64::from(x0)) as u32, (y1 - i64::from(y0)) as u32);
        // Sprite space: the rect after undoing the packing, rows bottom first.
        let (sw, sh) = if rotation == Rotation::Rotate90 {
            (h, w)
        } else {
            (w, h)
        };
        let coverage = match triangles {
            Some(t) => Some(self.coverage(sprite, t, placement, sw, sh)?),
            None => None,
        };
        let copying = self.reserve(u64::from(sw) * u64::from(sh))?;

        // The texel under sprite-space pixel (sx, sy): undo the rotation to get the crop
        // pixel (cx, cy), counted from the rect's bottom left, then find it in the top-down
        // texture.
        let src_row = texture.row();
        let texel = |cx: u32, cy: u32| {
            (texture.height - 1 - (y0 + cy)) as usize * src_row + (x0 + cx) as usize * 4
        };
        let crop = |sx: u32, sy: u32| match rotation {
            Rotation::Unrotated => (sx, sy),
            Rotation::FlipHorizontal => (w - 1 - sx, sy),
            Rotation::FlipVertical => (sx, h - 1 - sy),
            Rotation::Rotate180 => (w - 1 - sx, h - 1 - sy),
            // Undone counter-clockwise, as AssetStudio does; UnityPy turns the other way and
            // no real sample settles which is right.
            Rotation::Rotate90 => (w - 1 - sy, sx),
        };
        let out_row = sw as usize * 4;
        let mut rgba = vec![0u8; out_row * sh as usize];
        for (oy, line) in rgba.chunks_exact_mut(out_row.max(1)).enumerate() {
            // Output rows run top first; sprite space counts from the bottom.
            let sy = sh - 1 - oy as u32;
            let whole_row = coverage.is_none()
                && matches!(rotation, Rotation::Unrotated | Rotation::FlipVertical);
            if whole_row {
                let start = texel(0, crop(0, sy).1);
                line.copy_from_slice(&texture.rgba[start..start + out_row]);
                continue;
            }
            for (sx, px) in (0..sw).zip(line.chunks_exact_mut(4)) {
                let masked_out = coverage
                    .as_ref()
                    .is_some_and(|(c, _)| !c[sy as usize * sw as usize + sx as usize]);
                if masked_out {
                    continue;
                }
                let (cx, cy) = crop(sx, sy);
                let at = texel(cx, cy);
                px.copy_from_slice(&texture.rgba[at..at + 4]);
            }
        }
        copying.keep();
        if let Some((_, masking)) = coverage {
            masking.keep();
        }
        Ok(Image {
            width: sw,
            height: sh,
            rgba,
        })
    }

    /// Which pixels of the `w` x `h` sprite-space image (rows bottom first) the mesh covers:
    /// a pixel is kept when any of four sample points in it (see [`SAMPLES`]) lies in a
    /// triangle.
    /// Mesh vertices are in sprite units around the pivot. The work, one test per pixel of
    /// each triangle's bounding box, is counted before any is done and held to
    /// [`Limits::max_mask_work`] and the total.
    fn coverage(
        &self,
        sprite: &Sprite,
        triangles: &[[[f32; 2]; 3]],
        placement: &Placement,
        w: u32,
        h: u32,
    ) -> Result<(Vec<bool>, Reservation<'_>)> {
        let name = &sprite.name;
        let scale = sprite.pixels_to_units;
        let dx = sprite.rect.width * sprite.pivot[0] - placement.texture_rect_offset[0];
        let dy = sprite.rect.height * sprite.pivot[1] - placement.texture_rect_offset[1];
        let (wf, hf) = (w as f32, h as f32);
        // Triangles with area, mapped into pixels, with the pixel range each could touch.
        let mut boxes = Vec::with_capacity(triangles.len());
        let mut work = 0u64;
        for t in triangles {
            let p = t.map(|[x, y]| [x * scale + dx, y * scale + dy]);
            if p.iter().flatten().any(|v| !v.is_finite()) {
                return Err(Error::Invalid(format!(
                    "sprite {name:?} has a mesh vertex that is not a finite number"
                )));
            }
            let [a, b, c] = p;
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-9 {
                continue;
            }
            let lo = |i: usize| p.iter().map(|q| q[i]).fold(f32::INFINITY, f32::min);
            let hi = |i: usize| p.iter().map(|q| q[i]).fold(f32::NEG_INFINITY, f32::max);
            // Pixel x covers [x, x + 1]; it can overlap when x < max and x + 1 > min.
            let x_from = lo(0).floor().clamp(0.0, wf) as u32;
            let x_to = hi(0).ceil().clamp(0.0, wf) as u32;
            let y_from = lo(1).floor().clamp(0.0, hf) as u32;
            let y_to = hi(1).ceil().clamp(0.0, hf) as u32;
            if x_from < x_to && y_from < y_to {
                work += u64::from(x_to - x_from) * u64::from(y_to - y_from);
                boxes.push((p, x_from..x_to, y_from..y_to));
            } else {
                // Area, but none of it inside the image.
                boxes.push((p, 0..0, 0..0));
            }
        }
        if boxes.is_empty() {
            return Err(Error::Invalid(format!(
                "sprite {name:?} is tight-packed, and no triangle of its mesh has any area"
            )));
        }
        Error::limit(LimitKind::MaskWork, work, self.file.limits().max_mask_work)?;
        let reservation = self.reserve(work)?;

        let mut covered = vec![false; w as usize * h as usize];
        for ([a, b, c], xs, ys) in boxes {
            // On or inside all three edges, whichever way the triangle winds.
            let inside = |px: f32, py: f32| {
                let side = |p: [f32; 2], q: [f32; 2]| {
                    (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0])
                };
                let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
                let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                !(neg && pos)
            };
            for y in ys {
                for x in xs.clone() {
                    let (fx, fy) = (x as f32, y as f32);
                    if SAMPLES.iter().any(|&(u, v)| inside(fx + u, fy + v)) {
                        covered[y as usize * w as usize + x as usize] = true;
                    }
                }
            }
        }
        Ok((covered, reservation))
    }
}

/// Where a pixel is sampled for the mask: at the quarter points. Of the rules measured
/// against UnityPy's polygon fill on real sprites (pixel centre only, any overlap, these four,
/// two of five), keeping a pixel when any of these four is covered disagreed on the fewest
/// pixels.
const SAMPLES: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_new_checks_length() {
        assert!(Image::new(2, 2, vec![0; 16]).is_ok());
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
        assert!(Image::new(u32::MAX, u32::MAX, vec![]).is_err());
    }

    #[test]
    fn test_folder_of_a_bare_name_is_here() {
        assert_eq!(folder_of(Path::new("t.assets")), Path::new("."));
        assert_eq!(folder_of(Path::new("a/t.assets")), Path::new("a"));
        assert_eq!(folder_of(Path::new("/t.assets")), Path::new("/"));
    }
}
