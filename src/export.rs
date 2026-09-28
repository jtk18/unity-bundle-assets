//! Turning textures and sprites into images: find a sprite's placement (atlas or own
//! texture), decode the texture, then cut the sprite out in one pass: crop, undo packing
//! rotation, and mask tight-packed sprites to their mesh. Every image comes out top row first.

use crate::bundle::{self, Bundle, Entry};
use crate::decode;
use crate::serialized::{class, SerializedFile};
use crate::sprite::{Placement, Rotation, Sprite, SpriteAtlas};
use crate::texture::{is_console_platform, Texture2D};
use crate::{quoted, Error, LimitKind, Limits, Result, Shared, StreamKey};

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

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
            .field(
                "first_skipped",
                &self.skipped.first().map(|s| (s.path_id, &s.error)),
            )
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
/// then holds a decoded texture and the sprite cut from it, with its mask: at the default
/// 16384 x 16384, beyond the open file, about 1 GiB more to decode one texture of that size
/// and 2.25 GiB more to export a sprite from it. Lower [`Limits::max_texture_pixels`] to bound
/// it.
///
/// The work limit is spent and never given back: a long-running program that decodes the
/// same textures again and again should raise [`Limits::max_total_work`].
///
/// One `Assets` has one [`Limits::max_total_work`], shared with every `Assets` opened from the
/// same [`Bundle`]. Under that limit, which of several threads is refused depends on timing.
pub struct Assets {
    file: SerializedFile,
    streams: Streams,
    /// This file (or its folder, when only that is known), as named in errors about stream
    /// ranges.
    owner: Arc<str>,
    /// Atlases read so far, by path ID; one that fails to read is kept as its error, so it
    /// spoils only the sprites packed into it. Only atlases a sprite asks for are held.
    atlases: RwLock<HashMap<i64, Arc<OnceLock<AtlasResult>>>>,
    /// The most recently decoded texture, or why it could not be. Sprites sharing an atlas
    /// decode it once when exported in texture order (see [`Assets::sprites`]).
    cache: Option<(i64, std::result::Result<Image, Arc<Error>>)>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assets")
            .field("file", &self.file)
            .field("atlases", &self.atlases.read().map_or(0, |a| a.len()))
            .field("cached_texture", &self.cache.as_ref().map(|(id, _)| id))
            .field("work_done", &self.shared.work())
            .finish_non_exhaustive()
    }
}

type AtlasResult = std::result::Result<Arc<SpriteAtlas>, Arc<Error>>;

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
        let data = crate::file::read_limited(path, limits.max_file_size, |head, len| {
            if bundle::is_bundle(head) {
                bundle::check_head(head, len)
            } else {
                crate::serialized::check_head(head, len)
            }
        })?;
        let mut assets = Self::from_bytes(data, folder_of(path), limits)?;
        if matches!(assets.streams, Streams::Dir(_)) {
            assets.owner = path.display().to_string().into();
        }
        Ok(assets)
    }

    /// Open a serialized file or single-file bundle from its bytes. A serialized file's
    /// streamed textures are read from `stream_dir`; a bundle's come from the bundle, and
    /// `stream_dir` is not used.
    ///
    /// # Errors
    ///
    /// As [`Assets::open`].
    pub fn from_bytes(data: Vec<u8>, stream_dir: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        Error::limit(LimitKind::FileSize, data.len() as u64, limits.max_file_size)?;
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
                    .map(|e| quoted(e.path()))
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
            .ok_or_else(|| {
                Error::NotFound(format!("entry {path} in this bundle", path = quoted(path)))
            })?
            .clone();
        let file = SerializedFile::in_bundle(bundle.clone(), &entry)?;
        let shared = bundle.shared();
        Ok(Self::new(
            file,
            Streams::Bundle(bundle),
            entry.path().into(),
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
        let owner = dir.display().to_string().into();
        Ok(Self::new(
            file,
            Streams::Dir(dir),
            owner,
            Arc::new(Shared::default()),
        ))
    }

    fn new(file: SerializedFile, streams: Streams, owner: Arc<str>, shared: Arc<Shared>) -> Self {
        Self {
            file,
            streams,
            owner,
            atlases: RwLock::default(),
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

    fn reserve(&self, amount: u64) -> Result<()> {
        self.shared
            .reserve(amount, self.file.limits().max_total_work)
    }

    /// The atlas with this path ID, read on first use; `None` when this file has no atlas
    /// object with that ID. Two threads asking at once: one reads it, the other waits.
    fn atlas(&self, id: i64) -> Option<AtlasResult> {
        use std::sync::PoisonError;
        let known = self
            .atlases
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned();
        let cell = if let Some(cell) = known {
            cell
        } else {
            self.file
                .object(id)
                .filter(|o| o.class_id() == class::SPRITE_ATLAS)?;
            let mut atlases = self.atlases.write().unwrap_or_else(PoisonError::into_inner);
            atlases.entry(id).or_default().clone()
        };
        // One thread reads the atlas; any others asking at the same time wait for it.
        Some(
            cell.get_or_init(|| {
                let object = self
                    .file
                    .object(id)
                    .ok_or_else(|| Arc::new(Error::NotFound(format!("atlas {id}"))))?;
                SpriteAtlas::read(&self.file, object)
                    .map(Arc::new)
                    .map_err(Arc::new)
            })
            .clone(),
        )
    }

    /// Every sprite whose name passes `keep`, ordered by the texture holding its pixels so
    /// that [`Assets::export`] decodes each texture once. A sprite that cannot be read is
    /// listed in [`SpriteList::skipped`] rather than failing the rest.
    #[must_use]
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
            // An unreadable name is offered to `keep` as "", as `textures` does.
            let name = self.file.name(o);
            if !keep(name.as_deref().unwrap_or("")) {
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
    #[must_use]
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
                    "sprite {} is packed into an atlas in another file",
                    quoted(&sprite.name)
                )))
            };
        }
        let id = sprite.atlas.path_id;
        match self.atlas(id) {
            Some(Ok(atlas)) => match atlas.placement(&sprite.render_data_key) {
                Some(p) => Ok(p),
                None if own_texture => Ok(sprite.own),
                None => Err(Error::Invalid(format!(
                    "sprite {} is not in its atlas {id}",
                    quoted(&sprite.name)
                ))),
            },
            Some(Err(e)) => Err(Error::AtlasUnreadable {
                path_id: id,
                error: e,
            }),
            None if own_texture => Ok(sprite.own),
            None => Err(Error::NotFound(format!(
                "atlas {id} of sprite {}",
                quoted(&sprite.name)
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
                "texture {} was built for a console (platform {}), whose GPU tiling this \
                 crate does not undo",
                quoted(&texture.name),
                self.file.target_platform()
            )));
        }
        // A format this crate cannot decode is the reason, whatever the size.
        if !decode::is_supported(texture.format) {
            return Err(Error::UnsupportedTextureFormat {
                name: Some(texture.name),
                format: texture.format,
            });
        }
        let pixels = u64::from(texture.width) * u64::from(texture.height);
        let limit = self.file.limits().max_texture_pixels;
        Error::limit(LimitKind::TexturePixels, pixels, limit)?;
        // A streamed texture claims its range and reserves its work in one step, before any
        // of it is read: a texture refused its range takes no budget, and a range is claimed
        // only by a texture whose work was reserved. Once reserved, the work is kept: reading
        // starts at once, and work started is never given back.
        let mut claimed = false;
        let mut claim = |stream: StreamKey, start, end| {
            self.shared
                .claim_stream(stream, start, end, &self.owner, path_id, || {
                    self.reserve(pixels)
                })?;
            claimed = true;
            Ok(())
        };
        let data = match &self.streams {
            Streams::Dir(dir) => texture.data_claimed(dir, &mut claim)?,
            Streams::Bundle(bundle) => texture.data_in_claimed(bundle, &mut claim)?,
        };
        // Inline pixels claim nothing; they reserve here, with nothing done yet.
        if !claimed {
            self.reserve(pixels)?;
        }
        let (format, width, height) = (texture.format, texture.width, texture.height);
        let rgba = match data {
            Cow::Owned(v) => decode::decode_owned(format, width, height, v),
            Cow::Borrowed(b) => decode::decode(format, width, height, b),
        }
        .map_err(|e| match e {
            Error::Invalid(what) => {
                Error::Invalid(format!("texture {}: {what}", quoted(&texture.name)))
            }
            e => e,
        })?;
        Ok(Image {
            width,
            height,
            rgba,
        })
    }

    /// Export one sprite, decoding its texture unless it's the one decoded last. The decoded
    /// texture, or the reason it could not be decoded, stays cached until the next texture or
    /// [`Assets::clear_cache`], so a texture that fails is tried once, not once per sprite.
    ///
    /// # Errors
    ///
    /// As [`Assets::cut`]; when the texture cannot be decoded,
    /// [`Error::TextureUnreadable`] holding the reason.
    pub fn export(&mut self, sprite: &Sprite) -> Result<Image> {
        let placement = self.placement(sprite)?;
        let texture = texture_id_of(sprite, &placement)?;
        // The old texture is dropped before the next is decoded, so two are never held at
        // once (a `match` on `self.cache.take()` would keep it alive through the decode).
        let cached = self.cache.take();
        let decoded = match cached {
            Some((id, decoded)) if id == texture => decoded,
            other => {
                drop(other);
                self.decode_texture(texture).map_err(Arc::new)
            }
        };
        let image = match &decoded {
            Ok(image) => self.cut_at(sprite, &placement, image),
            Err(error) => Err(Error::TextureUnreadable {
                path_id: texture,
                error: error.clone(),
            }),
        };
        self.cache = Some((texture, decoded));
        image
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
                "sprite {name} keeps its alpha in a separate texture",
                name = quoted(name)
            )));
        }
        if !(placement.downscale.is_finite() && (placement.downscale - 1.0).abs() <= 1e-4) {
            return Err(Error::Unsupported(format!(
                "sprite {name} is in an atlas scaled by {}",
                placement.downscale,
                name = quoted(name)
            )));
        }
        let settings = placement.settings;
        let rotation = if settings.packed() {
            settings.rotation().ok_or_else(|| {
                Error::Unsupported(format!(
                    "sprite {name} has packing rotation {}, which Unity does not define",
                    settings.rotation_bits(),
                    name = quoted(name)
                ))
            })?
        } else {
            Rotation::Unrotated
        };
        let triangles = match (settings.tight(), &sprite.triangles) {
            (false, _) => None,
            (true, None) => {
                return Err(Error::Unsupported(format!(
                    "sprite {name} is cut out by its mesh, which is not one this crate reads",
                    name = quoted(name)
                )))
            }
            (true, Some(t)) if t.is_empty() => {
                return Err(Error::Invalid(format!(
                    "sprite {name} is cut out by its mesh, which is empty",
                    name = quoted(name)
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
        if x0 >= x1 || y0 >= y1 {
            return Err(Error::Invalid(format!(
                "sprite {name} covers {}x{} at {},{}: no whole pixel",
                r.width,
                r.height,
                r.x,
                r.y,
                name = quoted(name)
            )));
        }
        if !(0 <= x0 && x1 <= tw && 0 <= y0 && y1 <= th) {
            return Err(Error::Invalid(format!(
                "sprite {name} covers {}x{} at {},{}, not inside the {}x{} texture it was given",
                r.width,
                r.height,
                r.x,
                r.y,
                texture.width,
                texture.height,
                name = quoted(name)
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
        // The mask's work and the copy's, reserved together before either starts and kept
        // from then on: work done is never given back.
        let mask = match triangles {
            Some(t) => Some(self.plan_mask(sprite, t, placement, sw, sh)?),
            None => None,
        };
        let mask_work = mask.as_ref().map_or(0, |m| m.work);
        self.reserve(mask_work + u64::from(sw) * u64::from(sh))?;
        let coverage = mask.map(|m| m.fill(sw, sh)).transpose()?;

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
        let mut rgba = crate::zeroed(out_row * sh as usize)?;
        let whole_rows =
            coverage.is_none() && matches!(rotation, Rotation::Unrotated | Rotation::FlipVertical);
        for (oy, line) in rgba.chunks_exact_mut(out_row.max(1)).enumerate() {
            // Output rows run top first; sprite space counts from the bottom.
            let sy = sh - 1 - oy as u32;
            if whole_rows {
                let start = texel(0, crop(0, sy).1);
                line.copy_from_slice(&texture.rgba[start..start + out_row]);
                continue;
            }
            for (sx, px) in (0..sw).zip(line.chunks_exact_mut(4)) {
                let masked_out = coverage
                    .as_ref()
                    .is_some_and(|c| !c[sy as usize * sw as usize + sx as usize]);
                if masked_out {
                    continue;
                }
                let (cx, cy) = crop(sx, sy);
                let at = texel(cx, cy);
                px.copy_from_slice(&texture.rgba[at..at + 4]);
            }
        }
        Ok(Image {
            width: sw,
            height: sh,
            rgba,
        })
    }

    /// The mask for the `w` x `h` sprite-space image (rows bottom first): each triangle
    /// mapped into pixels, with the rows its box covers. Mesh vertices are in sprite units
    /// around the pivot. The work, a step for each row of each triangle and a test for each
    /// column of that row's span (see [`row_span`]), is counted here and held to
    /// [`Limits::max_mask_work`]; the caller reserves it before [`Mask::fill`].
    #[expect(
        clippy::many_single_char_names,
        reason = "a triangle's corners a, b, c and its points p, as in the geometry"
    )]
    fn plan_mask(
        &self,
        sprite: &Sprite,
        triangles: &[[[f32; 2]; 3]],
        placement: &Placement,
        w: u32,
        h: u32,
    ) -> Result<Mask> {
        let name = &sprite.name;
        let scale = sprite.pixels_to_units;
        let dx = sprite.rect.width * sprite.pivot[0] - placement.texture_rect_offset[0];
        let dy = sprite.rect.height * sprite.pivot[1] - placement.texture_rect_offset[1];
        let (wf, hf) = (w as f32, h as f32);
        // Triangles with area, mapped into pixels, with the pixel range each could touch.
        let mut boxes = Vec::with_capacity(triangles.len());
        let limit = self.file.limits().max_mask_work;
        for t in triangles {
            let p = t.map(|[x, y]| [x * scale + dx, y * scale + dy]);
            // Far past any image (16384 pixels at most), f32 has no pixel precision left.
            if p.iter().flatten().any(|v| !v.is_finite() || v.abs() > FAR) {
                return Err(Error::Invalid(format!(
                    "sprite {name} has a mesh vertex that is not a finite number within {FAR} \
                     pixels",
                    name = quoted(name)
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
                boxes.push((p, x_from..x_to, y_from..y_to));
            } else {
                // Area, but none of it inside the image.
                boxes.push((p, 0..0, 0..0));
            }
        }
        if boxes.is_empty() {
            return Err(Error::Invalid(format!(
                "sprite {name} is cut out by its mesh, and no triangle of it has any area",
                name = quoted(name)
            )));
        }
        // The rows alone, counted in constant time a triangle, before any span is worked out:
        // held to the mask limit and to what is left of the total.
        let rows: u64 = boxes
            .iter()
            .map(|(_, _, ys)| u64::from(ys.end - ys.start) * ROW_STEP)
            .sum();
        Error::limit(LimitKind::MaskWork, rows, limit)?;
        let total = self.file.limits().max_total_work;
        let done = self.shared.work();
        Error::limit(LimitKind::TotalWork, done.saturating_add(rows), total)?;
        // Then each row's span, stopping as soon as the total passes the limit. The spans
        // worked out by then are charged even when the mask is refused, so asking again does
        // not get them free.
        let mut work = 0u64;
        for (p, xs, ys) in &boxes {
            for y in ys.clone() {
                let span = row_span(p, y, xs.clone());
                work += ROW_STEP + u64::from(span.end - span.start);
                if work > limit {
                    self.shared.spend(work, self.file.limits().max_total_work);
                    return Err(Error::LimitExceeded {
                        kind: LimitKind::MaskWork,
                        value: work,
                        limit,
                    });
                }
            }
        }
        Ok(Mask { boxes, work })
    }
}

/// Triangles in pixel space, each with the pixel ranges its box covers.
struct Mask {
    boxes: Vec<TriangleBox>,
    work: u64,
}

/// A triangle's corners in pixels, and the columns and rows its bounding box covers.
type TriangleBox = ([[f32; 2]; 3], std::ops::Range<u32>, std::ops::Range<u32>);

impl Mask {
    /// Which pixels of the `w` x `h` image the mesh covers: a pixel is kept when any of four
    /// sample points in it (see [`SAMPLES`]) lies in a triangle.
    #[expect(
        clippy::many_single_char_names,
        reason = "a triangle's corners a, b, c, as in the geometry"
    )]
    fn fill(self, w: u32, h: u32) -> Result<Vec<bool>> {
        #[cfg(test)]
        tests::MASKS_FILLED.with(|n| n.set(n.get() + 1));
        let len = w as usize * h as usize;
        let mut covered = Vec::new();
        covered
            .try_reserve_exact(len)
            .map_err(|_| Error::OutOfMemory { bytes: len as u64 })?;
        covered.resize(len, false);
        for ([a, b, c], xs, ys) in self.boxes {
            // On or inside all three edges, whichever way the triangle winds.
            // In f64: an f32 edge test near a far vertex rounds by more than a sample's
            // distance from the edge.
            let triangle = [a, b, c];
            let [a, b, c] = triangle.map(|[x, y]| [f64::from(x), f64::from(y)]);
            let inside = |px: f64, py: f64| {
                let side = |p: [f64; 2], q: [f64; 2]| {
                    (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0])
                };
                let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
                let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
                let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
                !(neg && pos)
            };
            for y in ys {
                for x in row_span(&triangle, y, xs.clone()) {
                    let (fx, fy) = (f64::from(x), f64::from(y));
                    if SAMPLES.iter().any(|&(u, v)| inside(fx + u, fy + v)) {
                        covered[y as usize * w as usize + x as usize] = true;
                    }
                }
            }
        }
        Ok(covered)
    }
}

/// The columns of row `y`, within `columns`, that may hold a sample inside triangle `t`:
/// where the row's two sample lines (`y + 0.25`, `y + 0.75`) cross the triangle, widened by
/// a column each side for rounding. Only these are tested, so a thin triangle costs its
/// height, not its bounding box; every pixel with a sample inside is among them (for vertices
/// within [`FAR`] of the image, which the mesh is held to), so the mask is the same as
/// testing the whole box.
fn row_span(t: &[[f32; 2]; 3], y: u32, columns: std::ops::Range<u32>) -> std::ops::Range<u32> {
    // In f64, as the inside test is: with vertices held within `FAR`, its rounding is far
    // below the tolerance and the column of slack either side.
    let t = t.map(|[x, y]| [f64::from(x), f64::from(y)]);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for py in [f64::from(y) + 0.25, f64::from(y) + 0.75] {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (y0, y1) = if a[1] <= b[1] {
                (a[1], b[1])
            } else {
                (b[1], a[1])
            };
            // A little past each end, so rounding never drops a crossing.
            if py < y0 - 1e-6 || py > y1 + 1e-6 {
                continue;
            }
            let span = b[1] - a[1];
            if span.abs() < 1e-9 {
                // A flat edge on the line: both its ends.
                lo = lo.min(a[0].min(b[0]));
                hi = hi.max(a[0].max(b[0]));
            } else {
                let x = a[0] + (py - a[1]).clamp(y0 - a[1], y1 - a[1]) * (b[0] - a[0]) / span;
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    if lo > hi {
        return 0..0;
    }
    // Column x samples at x + 0.25 and x + 0.75.
    let from = ((lo - 0.75).floor() - 1.0).max(f64::from(columns.start));
    let to = ((hi - 0.25).floor() + 2.0).min(f64::from(columns.end));
    if from < to {
        from as u32..to as u32
    } else {
        0..0
    }
}

/// Mask work for one row of one triangle, besides a unit for each column tested: working out
/// its span, which is done twice (to count the work, then to fill), costs about as much as
/// sixteen column tests.
const ROW_STEP: u64 = 16;

/// Farthest a mesh vertex may lie from the image's corner, in pixels: four times the largest
/// texture. Much farther out, f32 rounding moves an edge by more than the column of slack
/// each row's span allows (measured from about a million pixels); no real sprite comes near.
const FAR: f32 = 65536.0;

/// Where a pixel is sampled for the mask: at the quarter points. Of the rules measured
/// against `UnityPy`'s polygon fill on real sprites (pixel centre only, any overlap, these four,
/// two of five), keeping a pixel when any of these four is covered disagreed on the fewest
/// pixels.
const SAMPLES: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

fn texture_id_of(sprite: &Sprite, placement: &Placement) -> Result<i64> {
    if placement.texture.is_null() {
        return Err(Error::Invalid(format!(
            "sprite {} has no texture",
            quoted(&sprite.name)
        )));
    }
    if placement.texture.file_id != 0 {
        return Err(Error::Unsupported(format!(
            "sprite {} uses a texture in another file",
            quoted(&sprite.name)
        )));
    }
    Ok(placement.texture.path_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        pub static MASKS_FILLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn test_a_mask_refused_for_the_total_is_never_built() {
        use crate::test_common::*;
        // A tight 4x4 sprite: decode 16, mask 77 (see the regression tests), copy 16.
        let tri = Mesh {
            vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
            indices: &[0, 1, 2],
            ..Mesh::BASE
        };
        let r = [0.0, 0.0, 4.0, 4.0];
        let s = sprite(
            false,
            false,
            "s",
            r,
            [0.0, 0.0],
            1,
            0,
            10,
            0,
            r,
            0,
            1.0,
            &tri,
        );
        let tex = texture(
            Layout::U2022_3,
            false,
            "t",
            4,
            4,
            format::RGBA32,
            &Pixels::Inline(&[9; 64]),
            &[],
        );
        let file = serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(10, TEXTURE_2D, tex), (1, SPRITE, s)],
        );
        for (limit, masks) in [(108, 0), (109, 1)] {
            let file = SerializedFile::parse_with(
                file.clone(),
                Limits::DEFAULT.with_max_total_work(limit),
            )
            .unwrap();
            let mut a = Assets::from_serialized(file, "").unwrap();
            let list = a.sprites(|_| true);
            MASKS_FILLED.with(|n| n.set(0));
            let _ = a.export(&list.sprites[0]);
            assert_eq!(
                MASKS_FILLED.with(std::cell::Cell::get),
                masks,
                "limit {limit}"
            );
        }
    }

    /// The documented rule for one sample, in f64 as the crate tests it.
    #[expect(
        clippy::many_single_char_names,
        reason = "a triangle's corners a, b, c, as in the geometry"
    )]
    fn sample_inside(t: &[[f32; 2]; 3], px: f64, py: f64) -> bool {
        let [a, b, c] = t.map(|[x, y]| [f64::from(x), f64::from(y)]);
        let side =
            |p: [f64; 2], q: [f64; 2]| (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0]);
        let d = [side(a, b), side(b, c), side(c, a)];
        !(d.iter().any(|&v| v < 0.0) && d.iter().any(|&v| v > 0.0))
    }

    #[test]
    fn test_row_spans_hold_every_sample_inside_at_16k() {
        // The round-6 review's case: an apex a few units in the last place from a sample line,
        // 16k pixels up, with the other corners far below. In f32 the inside test itself
        // rounded, so samples just outside counted as inside and their spans missed them;
        // in f64 a sample just above the apex is outside and one just below is inside, and
        // the span must hold every one that is.
        let mut seed = 0x1234_9876_5555_aaaau64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut tried = 0;
        for _ in 0..300_000 {
            let y = 8192 + (rnd() % 8000) as u32;
            let x = (rnd() % 16000) as u32 + 100;
            let sx = x as f32 + if rnd() % 2 == 0 { 0.25 } else { 0.75 };
            let sy = y as f32 + 0.25;
            let ulps = 1 + (rnd() % 6) as u32;
            let ay = f32::from_bits(if rnd() % 2 == 0 {
                sy.to_bits() - ulps
            } else {
                sy.to_bits() + ulps
            });
            let ax = f32::from_bits((i64::from(sx.to_bits()) + (rnd() % 7) as i64 - 3) as u32);
            let corner = |rnd: &mut dyn FnMut() -> u64| {
                [
                    (rnd() % 16384) as f32 + (rnd() % 1000) as f32 * 1e-3,
                    (rnd() % 64) as f32,
                ]
            };
            let t = [[ax, ay], corner(&mut rnd), corner(&mut rnd)];
            if !sample_inside(&t, f64::from(sx), f64::from(sy)) {
                continue;
            }
            tried += 1;
            let lo = t.iter().map(|q| q[0]).fold(f32::INFINITY, f32::min).floor() as u32;
            let hi = t
                .iter()
                .map(|q| q[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil() as u32;
            let span = row_span(&t, y, lo..hi.min(16384));
            assert!(
                span.contains(&x),
                "pixel ({x},{y}) missed by {span:?}: {t:?}"
            );
        }
        assert!(tried > 1000, "{tried}");
    }

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
