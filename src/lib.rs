//! Read Unity serialized asset files and asset bundles, and export their sprites and
//! textures as RGBA images, in pure Rust.
//!
//! [`Assets`] is the entry point. It opens a serialized file (`*.assets`, `level*`) or an
//! asset bundle holding one, lists its sprites and textures, and decodes them. Every image it
//! returns is RGBA8, top row first.
//!
//! ```no_run
//! # fn main() -> Result<(), unity_bundle_assets::Error> {
//! use unity_bundle_assets::Assets;
//!
//! // Sprites, cut out of their textures or atlases.
//! let mut assets = Assets::open("Game_Data/sharedassets0.assets")?;
//! let list = assets.sprites(|name| name.starts_with("Icon_"));
//! for sprite in &list.sprites {
//!     let image = assets.export(sprite)?;
//! }
//! for skipped in &list.skipped {
//!     eprintln!("{}: {}", skipped.path_id, skipped.error);
//! }
//!
//! // Whole textures, from a bundle.
//! let assets = Assets::open("assetbundles/characters")?;
//! for texture in assets.textures(|_| true) {
//!     let image = assets.decode_texture(texture.path_id)?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The lower layers are public too: [`Bundle`] for the container, [`SerializedFile`] for the
//! object table, [`Texture2D`], [`Sprite`] and [`SpriteAtlas`] for single objects, and
//! [`decode`] for pixel formats.
//!
//! # Untrusted input
//!
//! Files are treated as hostile:
//!
//! - Counts and lengths are checked against the bytes present before they size an
//!   allocation, and every string read is at most 4 KiB of the file (names at most three
//!   times that as text, invalid UTF-8 shown as U+FFFD; versions and the paths used to find
//!   data must be UTF-8).
//!   Objects in a file, entries in a bundle, a sprite's sub-meshes, and the stream ranges
//!   textures read may not overlap, so one blob cannot be decoded many times over; stream
//!   ranges are compared by the bytes they reach (on Unix; by lower-cased ASCII name
//!   elsewhere), however the path to them is spelled.
//! - Sizes the data alone cannot bound are held to [`Limits`]: file size, decompressed bundle
//!   size (the parsed directory and LZMA's working memory included), object count, decoded
//!   pixels, sprite meshes, and the total work spent decoding, cutting and masking, shared by
//!   everything opened from one file or bundle. Work is reserved before it starts and kept once
//!   reserved. The limits bound work, not peak memory: at the default 16384 x 16384, beyond
//!   the open file, up to about 1.75 GiB more to decode one texture of that size and 2.25 GiB
//!   more to export a sprite from it.
//! - Streamed pixels are read only from the same bundle, or from a `.resS` / `.resource` file
//!   directly beside the asset file that is a regular file, not a symbolic link, not a Windows
//!   device name, and (on Unix) has no other hard links.
//! - Strings from the file are quoted in error messages, so printing an error cannot send
//!   control sequences to a terminal. The strings themselves (names, paths, versions) are
//!   returned as found; escape them before printing.
//!
//! [`Assets`] enforces every limit. Used directly, [`Bundle`] and [`SerializedFile`] apply
//! their own, [`Texture2D`] refuses data too short for its size but claims no ranges, and
//! [`decode::decode`] applies none; none of them tracks total work.
//!
//! An error [`Assets::export`] or an atlas passes on wraps its cause; [`Error::root`] unwraps
//! it.
//!
//! Malformed data is meant to give an [`Error`] rather than a panic. `tests/fuzz.rs` checks
//! that over mutated files; it is evidence, not proof.

mod bundle;
pub mod decode;
mod export;
mod file;
mod reader;
mod serialized;
mod sprite;
mod texture;

pub use bundle::{is_bundle, Bundle, Entry};
pub use export::{Assets, Image, SkippedSprite, SpriteList, TextureInfo};
pub use serialized::{class, External, ObjectInfo, SerializedFile, SerializedType};
pub use sprite::{PPtr, Placement, Rect, RenderDataKey, Rotation, Settings, Sprite, SpriteAtlas};
pub use texture::{is_console_platform, StreamingInfo, Texture2D};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// The README's examples, compiled as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

/// Everything that can go wrong reading a file.
///
/// Strings that came from a file are quoted (`{:?}`) when displayed, and cut to 64
/// characters; the fields themselves (and so `Debug`) hold them whole, up to 12 KiB. Each
/// message is complete: an underlying cause is part of it rather than a
/// separate [`std::error::Error::source`] (see [`Error::root`] for the wrapped ones).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The data ended before a read finished. The offset counts from the start of the
    /// structure being read (an object, a header, a block table), not the file.
    #[error("unexpected end of data at byte {0}")]
    Truncated(usize),
    /// A length or count that cannot fit in the bytes that follow it, or a length-prefixed
    /// string longer than the 4 KiB this crate reads (a NUL-terminated one that long is
    /// [`Error::Invalid`]). The offset counts as for [`Error::Truncated`].
    #[error("bad length {len} at byte {at}")]
    #[non_exhaustive]
    BadLength {
        /// Byte offset of the length field.
        at: usize,
        /// The length it claimed.
        len: i64,
    },
    /// The data is not a Unity file this crate recognises.
    #[error("not a Unity file: {0}")]
    NotUnity(String),
    /// A Unity file or feature this crate does not read yet: a format or engine version,
    /// container, compression or packing. The message names it.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A texture in a pixel format this crate does not decode.
    #[error("{}unsupported texture format {format}", texture_label(name.as_deref()))]
    #[non_exhaustive]
    UnsupportedTextureFormat {
        /// The texture's name, when known.
        name: Option<String>,
        /// Unity's `TextureFormat` value.
        format: i32,
    },
    /// An encrypted asset bundle.
    #[error("encrypted asset bundle")]
    Encrypted,
    /// A size or amount of work over one of the [`Limits`] in force.
    #[error("{kind} is {value}, over the limit of {limit}")]
    #[non_exhaustive]
    LimitExceeded {
        /// Which limit.
        kind: LimitKind,
        /// The size or work it measured.
        value: u64,
        /// The limit.
        limit: u64,
    },
    /// No object or entry by that ID or path.
    #[error("not found: {0}")]
    NotFound(String),
    /// An object that exists but is not of the class asked for.
    #[error("object {path_id} is class {found}, not {expected}")]
    #[non_exhaustive]
    WrongClass {
        /// The object's path ID.
        path_id: i64,
        /// Its class ID.
        found: i32,
        /// The class ID asked for.
        expected: i32,
    },
    /// A sprite atlas that could not be read, which spoils the sprites packed into it.
    #[error("sprite atlas {path_id} could not be read: {error}")]
    #[non_exhaustive]
    AtlasUnreadable {
        /// The atlas's path ID.
        path_id: i64,
        /// Why; shared by every sprite in the atlas.
        error: Arc<Self>,
    },
    /// A texture [`Assets::export`] could not decode, which spoils the sprites cut from it.
    #[error("texture {path_id} could not be decoded: {error}")]
    #[non_exhaustive]
    TextureUnreadable {
        /// The texture's path ID.
        path_id: i64,
        /// Why; shared by every sprite exported from the texture.
        error: Arc<Self>,
    },
    /// A texture stored with no pixels, as dynamic font textures are; it is filled at run
    /// time.
    #[error("texture {} is empty (0x0)", quoted(.0))]
    EmptyTexture(String),
    /// Data that contradicts itself, or a layout this crate misread.
    #[error("invalid file: {0}")]
    Invalid(String),
    /// An argument the caller built that contradicts itself, such as an [`Image`] whose
    /// pixels do not match its size.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Memory for data within the limits could not be had.
    #[error("out of memory allocating {bytes} bytes")]
    #[non_exhaustive]
    OutOfMemory {
        /// The allocation asked for.
        bytes: u64,
    },
    /// Reading a file from disk failed.
    #[error("{}: {error}", quoted_path(&path.display().to_string()))]
    #[non_exhaustive]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: std::io::Error,
    },
}

fn texture_label(name: Option<&str>) -> String {
    name.map_or_else(String::new, |n| format!("texture {n}: ", n = quoted(n)))
}

impl Error {
    /// The error behind this one: through [`Error::AtlasUnreadable`] and
    /// [`Error::TextureUnreadable`], which wrap the reason an atlas or texture could not be
    /// read, to that reason. Match on this to handle, say, an unsupported texture format
    /// the same whether [`Assets::decode_texture`] or [`Assets::export`] reported it.
    #[must_use]
    pub fn root(&self) -> &Self {
        match self {
            Self::AtlasUnreadable { error, .. } | Self::TextureUnreadable { error, .. } => {
                error.root()
            }
            e => e,
        }
    }

    /// The I/O error behind this one, if any (see [`Error::root`]), for testing its kind: a
    /// missing stream file is [`std::io::ErrorKind::NotFound`]. Errors carry no
    /// [`std::error::Error::source`]: each message is complete on its own.
    #[must_use]
    pub fn io_error(&self) -> Option<&std::io::Error> {
        match self.root() {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }

    pub(crate) fn io(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> Self + '_ {
        move |error| Self::Io {
            path: path.to_path_buf(),
            error,
        }
    }

    pub(crate) const fn limit(kind: LimitKind, value: u64, limit: u64) -> Result<()> {
        if value > limit {
            return Err(Self::LimitExceeded { kind, value, limit });
        }
        Ok(())
    }
}

/// Which of the [`Limits`] a [`Error::LimitExceeded`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LimitKind {
    /// [`Limits::max_file_size`].
    FileSize,
    /// [`Limits::max_decompressed`].
    Decompressed,
    /// [`Limits::max_objects`].
    Objects,
    /// [`Limits::max_texture_pixels`].
    TexturePixels,
    /// [`Limits::max_sprite_triangles`].
    SpriteTriangles,
    /// [`Limits::max_total_triangles`].
    TotalTriangles,
    /// [`Limits::max_mask_work`].
    MaskWork,
    /// [`Limits::max_total_work`].
    TotalWork,
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::FileSize => "file size",
            Self::Decompressed => "decompressed bundle size",
            Self::Objects => "object count",
            Self::TexturePixels => "texture pixels",
            Self::SpriteTriangles => "sprite mesh triangles",
            Self::TotalTriangles => "sprite mesh triangles in this list",
            Self::MaskWork => "sprite mask work",
            Self::TotalWork => "total work",
        })
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Ceilings on work and memory that the data alone cannot bound. The defaults sit well above
/// what the games this crate was tested on use; lower them for batch tools that open files
/// from strangers.
///
/// ```
/// use unity_bundle_assets::Limits;
/// let limits = Limits::DEFAULT.with_max_decompressed(256 << 20);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest file [`Assets::open`] and friends will read, in bytes. Default 2 GiB.
    pub max_file_size: u64,
    /// Largest decompressed size of a bundle in bytes, counting the parsed directory at its
    /// in-memory size. Default 1 GiB.
    pub max_decompressed: u64,
    /// Most objects in one serialized file, and across the files opened from one bundle.
    /// Default 4,194,304.
    pub max_objects: u64,
    /// Most pixels in one texture this crate will decode. Default 16384 x 16384, Unity's
    /// own maximum: beyond the open file, up to about 1.75 GiB more to decode one texture of
    /// that size and 2.25 GiB more to export a sprite from it.
    pub max_texture_pixels: u64,
    /// Most triangles in one sprite's mesh. Default 65,536.
    pub max_sprite_triangles: u64,
    /// Most triangles across the sprites one [`Assets::sprites`] call returns. Default
    /// 4,194,304.
    pub max_total_triangles: u64,
    /// Most work spent masking one tight-packed sprite: 16 units for each mesh triangle and
    /// each row a triangle crosses, and one for each column tested. Default 2^29, enough for
    /// a two-triangle mesh over a 16384 x 16384 sprite.
    pub max_mask_work: u64,
    /// Most work decoding, masking and cutting by one [`Assets`], or by every `Assets` opened
    /// from one [`Bundle`], in units of about one pixel's: a unit for each pixel decoded
    /// (whole 4x4 blocks for block formats) or copied (two for a quarter-turned sprite), 64
    /// for each decode or cut asked for, refused or not, a unit for each byte of a texture
    /// name past 64, 8192 more for each stream file opened, and the mask work. Opening and
    /// listing are not counted. Default 2^34, under a minute of CPU.
    pub max_total_work: u64,
}

impl Limits {
    /// The defaults, as a constant.
    pub const DEFAULT: Self = Self {
        max_file_size: 2 << 30,
        max_decompressed: 1 << 30,
        max_objects: 1 << 22,
        max_texture_pixels: 16384 * 16384,
        max_sprite_triangles: 1 << 16,
        max_total_triangles: 1 << 22,
        max_mask_work: 1 << 29,
        max_total_work: 1 << 34,
    };

    /// With [`Limits::max_file_size`] set.
    #[must_use]
    pub const fn with_max_file_size(mut self, v: u64) -> Self {
        self.max_file_size = v;
        self
    }

    /// With [`Limits::max_decompressed`] set.
    #[must_use]
    pub const fn with_max_decompressed(mut self, v: u64) -> Self {
        self.max_decompressed = v;
        self
    }

    /// With [`Limits::max_objects`] set.
    #[must_use]
    pub const fn with_max_objects(mut self, v: u64) -> Self {
        self.max_objects = v;
        self
    }

    /// With [`Limits::max_texture_pixels`] set.
    #[must_use]
    pub const fn with_max_texture_pixels(mut self, v: u64) -> Self {
        self.max_texture_pixels = v;
        self
    }

    /// With [`Limits::max_sprite_triangles`] set.
    #[must_use]
    pub const fn with_max_sprite_triangles(mut self, v: u64) -> Self {
        self.max_sprite_triangles = v;
        self
    }

    /// With [`Limits::max_total_triangles`] set.
    #[must_use]
    pub const fn with_max_total_triangles(mut self, v: u64) -> Self {
        self.max_total_triangles = v;
        self
    }

    /// With [`Limits::max_mask_work`] set.
    #[must_use]
    pub const fn with_max_mask_work(mut self, v: u64) -> Self {
        self.max_mask_work = v;
        self
    }

    /// With [`Limits::max_total_work`] set.
    #[must_use]
    pub const fn with_max_total_work(mut self, v: u64) -> Self {
        self.max_total_work = v;
        self
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Objects counted across a bundle's serialized files.
#[derive(Debug, Default)]
struct ObjectCount {
    total: u64,
    /// The entries (by offset) already counted: opening one again counts nothing more.
    entries: HashSet<usize>,
}

/// Work done and stream ranges read, shared by everything opened from one file or bundle, so
/// opening a bundle's files one by one does not multiply the budget.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    work: AtomicU64,
    /// Objects in the serialized files opened from a bundle, held to
    /// [`Limits::max_objects`] across all of them.
    objects: Mutex<ObjectCount>,
    /// Stream ranges already read, by the bytes they name (not by how a texture spelled the
    /// path to them): start -> the claim.
    streams: Mutex<HashMap<StreamKey, BTreeMap<u64, Claim>>>,
}

/// What a stream range is a range of. Two spellings that reach the same bytes get one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum StreamKey {
    /// The bundle's decompressed data; ranges are absolute offsets into it.
    Bundle,
    /// A file beside the asset file.
    File(crate::file::FileId),
}

/// Who read a stream range.
#[derive(Debug)]
struct Claim {
    end: u64,
    /// The file the texture is in, shared by all its claims.
    owner: Arc<str>,
    texture: i64,
}

impl Shared {
    /// Take `amount` of work from the total before doing it, or refuse without taking any.
    /// Work taken is never given back: it is taken only when the work is about to start.
    pub fn reserve(&self, amount: u64, limit: u64) -> Result<()> {
        self.work
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |w| {
                w.checked_add(amount).filter(|&t| t <= limit)
            })
            .map_err(|w| Error::LimitExceeded {
                kind: LimitKind::TotalWork,
                value: w.saturating_add(amount),
                limit,
            })?;
        Ok(())
    }

    pub fn work(&self) -> u64 {
        self.work.load(Ordering::Relaxed)
    }

    /// How many more objects the entry at `entry` may hold under `limit`: all of it for an
    /// entry already counted, what the others left otherwise.
    pub fn objects_left(&self, entry: usize, limit: u64) -> u64 {
        let counted = self.objects.lock().unwrap_or_else(PoisonError::into_inner);
        if counted.entries.contains(&entry) {
            limit
        } else {
            limit.saturating_sub(counted.total)
        }
    }

    /// Count the `n` objects of the entry at `entry` against `limit`, once per entry, or
    /// refuse without counting them. The error gives the bundle's total and the limit.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the check and the count are one step"
    )]
    pub fn count_objects(&self, entry: usize, n: u64, limit: u64) -> Result<()> {
        let mut counted = self.objects.lock().unwrap_or_else(PoisonError::into_inner);
        if counted.entries.contains(&entry) {
            return Ok(());
        }
        let total = counted.total.saturating_add(n);
        Error::limit(LimitKind::Objects, total, limit)?;
        counted.total = total;
        counted.entries.insert(entry);
        Ok(())
    }

    /// Record that texture `texture` of `owner` reads `start..end` of `stream`, and reserve
    /// its work with `reserve`, as one step under the claims lock: a range another texture
    /// already read any part of is refused (one blob decoded many times over), and a claim is
    /// recorded only once its work is reserved, so no texture is ever refused for a range
    /// whose owner was itself refused. The same texture reading its own range again is fine.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the check, the reservation and the insert are one step"
    )]
    pub fn claim_stream<R>(
        &self,
        stream: StreamKey,
        start: u64,
        end: u64,
        owner: &Arc<str>,
        texture: i64,
        reserve: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ranges = streams.entry(stream).or_default();
        if let Some((&s, earlier)) = ranges.range(..end).next_back() {
            let same = s == start
                && earlier.end == end
                && earlier.texture == texture
                && earlier.owner == *owner;
            if same {
                return reserve();
            }
            if earlier.end > start {
                return Err(Error::Invalid(format!(
                    "texture {texture} of {} streams bytes that texture {} of {} already read",
                    quoted_path(owner),
                    earlier.texture,
                    quoted_path(&earlier.owner)
                )));
            }
        }
        let reserved = reserve()?;
        ranges.insert(
            start,
            Claim {
                end,
                owner: owner.clone(),
                texture,
            },
        );
        Ok(reserved)
    }
}

/// `len` zero bytes, or [`Error::OutOfMemory`] rather than an abort when they cannot be had.
pub(crate) fn zeroed(len: usize) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| Error::OutOfMemory { bytes: len as u64 })?;
    v.resize(len, 0);
    Ok(v)
}

/// A string from a file, for an error message: quoted with `{:?}`, so it cannot send control
/// sequences to a terminal, and cut to its first 64 characters, so a message stays short
/// whatever the file holds.
pub(crate) fn quoted(s: &str) -> String {
    const SHOWN: usize = 64;
    let mut chars = s.char_indices();
    match chars.nth(SHOWN) {
        Some((cut, _)) => format!("{:?}... ({} bytes)", &s[..cut], s.len()),
        None => format!("{s:?}"),
    }
}

/// A path for an error message: as [`quoted`], but keeping its last 64 characters, where the
/// file's name is.
pub(crate) fn quoted_path(s: &str) -> String {
    const SHOWN: usize = 64;
    let count = s.chars().count();
    if count <= SHOWN {
        return format!("{s:?}");
    }
    let cut = s.char_indices().nth(count - SHOWN).map_or(0, |(i, _)| i);
    format!("({} bytes) ...{:?}", s.len(), &s[cut..])
}

/// Longest engine version string accepted; real ones are under 20 bytes.
const MAX_VERSION_LEN: usize = 32;

/// Refuse an engine version string that is too long or holds anything but the letters,
/// digits, dots and dashes versions are made of.
pub(crate) fn check_version_string(version: &str, what: &str) -> Result<()> {
    let plausible = version.len() <= MAX_VERSION_LEN
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if plausible {
        Ok(())
    } else {
        Err(Error::NotUnity(format!(
            "{what} engine version is not a version ({} bytes)",
            version.len()
        )))
    }
}

/// An engine version: `2022.3.62f3` is `[2022, 3, 62]`, release type `f`, build 3. Anything
/// that does not parse is `[0, 0, 0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Version {
    pub numbers: [u32; 3],
    /// `a` alpha, `b` beta, `f` final, `p` patch, `c` China, `t` Tuanjie, `x` experimental;
    /// `\0` if none.
    pub kind: char,
    /// The number after the release type.
    pub build: u32,
}

impl Version {
    pub fn parse(version: &str) -> Self {
        let mut numbers = [0; 3];
        let mut parts = version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty());
        for slot in &mut numbers {
            *slot = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        }
        let rest = version.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        let kind = rest.chars().next().unwrap_or('\0');
        let build = rest
            .get(kind.len_utf8()..)
            .unwrap_or("")
            .bytes()
            .take_while(u8::is_ascii_digit)
            .try_fold(0u32, |n, d| {
                n.checked_mul(10)?.checked_add(u32::from(d - b'0'))
            })
            .unwrap_or(0);
        Self {
            numbers,
            kind,
            build,
        }
    }

    pub fn stripped(&self) -> bool {
        self.numbers == [0, 0, 0]
    }

    /// Whether this is at least `numbers` of type `kind` build `build`: alphas come before
    /// betas, then China builds, finals and patches, as `UnityPy` orders them.
    pub fn at_least(&self, numbers: [u32; 3], kind: char, build: u32) -> bool {
        let rank = |k: char| match k {
            'a' => 0,
            'b' => 1,
            'c' => 2,
            'p' => 4,
            _ => 3,
        };
        (self.numbers, rank(self.kind), self.build) >= (numbers, rank(kind), build)
    }
}

/// The newest engine release whose layouts this crate was checked against. Files from later
/// releases are refused rather than guessed at.
pub(crate) const NEWEST_KNOWN: [u32; 2] = [6000, 4];

/// Refuse object layouts from releases outside `[oldest, NEWEST_KNOWN]`, alpha builds (whose
/// layouts shift between builds), engine variants whose layouts are unchecked, and files whose
/// engine version was stripped.
pub(crate) fn check_release(version: &str, what: &str, oldest: [u32; 3]) -> Result<()> {
    let v = Version::parse(version);
    if v.stripped() {
        return Err(Error::Unsupported(format!(
            "{what} from a file whose engine version was stripped ({version:?})"
        )));
    }
    if v.numbers < oldest {
        return Err(Error::Unsupported(format!(
            "{what} from Unity {version:?} (needs {}.{} or later)",
            oldest[0], oldest[1]
        )));
    }
    if [v.numbers[0], v.numbers[1]] > NEWEST_KNOWN {
        return Err(Error::Unsupported(format!(
            "{what} from Unity {version:?}, newer than the layouts this crate knows (up to {}.{})",
            NEWEST_KNOWN[0], NEWEST_KNOWN[1]
        )));
    }
    match v.kind {
        'a' => Err(Error::Unsupported(format!(
            "{what} from Unity {version:?}, an alpha build; layouts change between alphas"
        ))),
        't' | 'x' => Err(Error::Unsupported(format!(
            "{what} from Unity {version:?}, an engine variant whose layouts are unchecked"
        ))),
        _ => Ok(()),
    }
}

/// The integration tests' file builders, for unit tests that need a whole file.
#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod test_common;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quoted_keeps_the_start_and_quoted_path_the_end() {
        assert_eq!(quoted("abc"), "\"abc\"");
        let long: String = ('a'..='z').cycle().take(100).collect();
        assert_eq!(quoted(&long), format!("{:?}... (100 bytes)", &long[..64]));
        assert_eq!(quoted(&long[..64]), format!("{:?}", &long[..64]));
        assert_eq!(
            quoted(&long[..65]),
            format!("{:?}... (65 bytes)", &long[..64])
        );
        assert_eq!(quoted_path(&long[..64]), format!("{:?}", &long[..64]));
        assert_eq!(
            quoted_path(&long),
            format!("(100 bytes) ...{:?}", &long[36..])
        );
        assert_eq!(
            quoted_path(&long[..65]),
            format!("(65 bytes) ...{:?}", &long[1..65])
        );
        // Characters, not bytes.
        let wide = "\u{e9}".repeat(70);
        assert!(quoted(&wide).ends_with("... (140 bytes)"));
        assert!(quoted_path(&wide).starts_with("(140 bytes) ..."));
    }

    #[test]
    fn test_version_parts_default_to_zero() {
        assert_eq!(Version::parse("2018.4").numbers, [2018, 4, 0]);
        assert_eq!(Version::parse("5").numbers, [5, 0, 0]);
        assert!(Version::parse("").stripped());
        assert!(Version::parse("0.0.0").stripped());
        assert!(!Version::parse("2018.4").stripped());
        for (text, build) in [
            ("2022.3.1f12", 12),
            ("2022.3.1f9", 9),
            ("2022.3.1b123", 123),
            ("2022.3.1", 0),
        ] {
            assert_eq!(Version::parse(text).build, build, "{text}");
        }
    }

    #[test]
    fn test_version_parse() {
        let v = Version::parse("2022.3.62f3");
        assert_eq!((v.numbers, v.kind, v.build), ([2022, 3, 62], 'f', 3));
        let v = Version::parse("6000.5.0a5");
        assert_eq!((v.numbers, v.kind, v.build), ([6000, 5, 0], 'a', 5));
        let v = Version::parse("2020.3.48f1c1");
        assert_eq!((v.numbers, v.kind, v.build), ([2020, 3, 48], 'f', 1));
        assert!(Version::parse("0.0.0").stripped());
        assert!(Version::parse("").stripped());
    }

    #[test]
    fn test_version_order() {
        let order = [
            "2022.3.1a9",
            "2022.3.1b2",
            "2022.3.1c1",
            "2022.3.1f1",
            "2022.3.1p1",
            "2022.3.2a1",
        ];
        for w in order.windows(2) {
            let later = Version::parse(w[1]);
            let earlier = Version::parse(w[0]);
            assert!(
                later.at_least(earlier.numbers, earlier.kind, earlier.build),
                "{w:?}"
            );
            assert!(
                !earlier.at_least(later.numbers, later.kind, later.build),
                "{w:?}"
            );
        }
        assert_eq!(Version::parse("2022.3.99999999999f1").numbers[2], 0);
        assert_eq!(Version::parse("2022.3.1f99999999999").build, 0);
        let at = |v: &str| Version::parse(v).at_least([2022, 2, 0], 'b', 3);
        assert!(!at("2022.2.0b2"));
        assert!(at("2022.2.0b3"));
        assert!(at("2022.2.0f1"));
        assert!(at("2022.2.1b1"));
        assert!(!at("2022.1.24f1"));
        assert!(!at("2022.2.0a18"));
    }

    #[test]
    fn test_check_release() {
        assert!(check_release("2022.3.62f3", "x", [5, 5, 0]).is_ok());
        assert!(check_release("2019.1.0b5", "x", [2019, 1, 0]).is_ok());
        assert!(check_release("2020.3.48f1c1", "x", [5, 5, 0]).is_ok());
        for refused in [
            "2023.2.0a17",
            "6000.5.0f1",
            "5.4.0f1",
            "2022.3.2t3",
            "2022.3.0x1",
        ] {
            assert!(check_release(refused, "x", [5, 5, 0]).is_err(), "{refused}");
        }
        assert!(check_release("6000.4.3f1", "x", [5, 5, 0]).is_ok());
        let stripped = check_release("0.0.0", "x", [5, 5, 0])
            .unwrap_err()
            .to_string();
        assert!(stripped.contains("stripped"), "{stripped}");
    }

    #[test]
    fn test_version_strings() {
        assert!(check_version_string("2022.3.62f3", "x").is_ok());
        assert!(check_version_string("5.x.x", "x").is_ok());
        assert!(check_version_string("", "x").is_ok());
        assert!(check_version_string(&"1".repeat(33), "x").is_err());
        assert!(check_version_string("2022.3\u{1}", "x").is_err());
    }

    #[test]
    fn test_defaults_are_as_documented() {
        let d = Limits::default();
        assert_eq!(d, Limits::DEFAULT);
        assert_eq!(d.max_file_size, 2 << 30);
        assert_eq!(d.max_decompressed, 1 << 30);
        assert_eq!(d.max_objects, 1 << 22);
        assert_eq!(d.max_texture_pixels, 16384 * 16384);
        assert_eq!(d.max_sprite_triangles, 1 << 16);
        assert_eq!(d.max_total_triangles, 1 << 22);
        assert_eq!(d.max_mask_work, 1 << 29);
        assert_eq!(d.max_total_work, 1 << 34);
    }

    #[test]
    fn test_error_messages() {
        let e = Error::UnsupportedTextureFormat {
            name: Some("t".into()),
            format: 25,
        };
        assert_eq!(
            e.to_string(),
            "texture \"t\": unsupported texture format 25"
        );
        let e = Error::UnsupportedTextureFormat {
            name: None,
            format: 25,
        };
        assert_eq!(e.to_string(), "unsupported texture format 25");
        let e = Error::Io {
            path: "/x".into(),
            error: std::io::Error::other("boom"),
        };
        assert_eq!(e.to_string(), "\"/x\": boom");
        assert!(std::error::Error::source(&e).is_none());
    }

    #[test]
    fn test_assets_are_send_and_sync() {
        fn both<T: Send + Sync>() {}
        both::<Assets>();
        both::<Bundle>();
        both::<SerializedFile>();
        both::<Error>();
    }
}
