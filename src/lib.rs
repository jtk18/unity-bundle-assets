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
//!   allocation. Objects in a file, entries in a bundle, and the stream ranges textures read
//!   may not overlap, so one blob cannot be decoded many times over.
//! - Sizes the data alone cannot bound are held to [`Limits`]: file size, decompressed bundle
//!   size (the parsed directory included), object count, decoded pixels, sprite meshes, and
//!   the total work spent decoding, cutting and masking, shared by everything opened from one
//!   file or bundle. They bound work, not peak memory: at the default 16384 x 16384, one
//!   decoded texture can hold 1 to 2 GiB and a sprite exported from it 2 to 3 GiB.
//! - Streamed pixels are read only from the same bundle, or from a `.resS` / `.resource` file
//!   directly beside the asset file that is a regular file, not a symbolic link, and has no
//!   other hard links.
//! - Strings from the file are quoted in error messages, so printing an error cannot send
//!   control sequences to a terminal. The strings themselves (names, paths, versions) are
//!   returned as found; escape them before printing.
//!
//! [`Limits`] are enforced by [`Assets`] and by the parsers ([`Bundle::parse_with`],
//! [`SerializedFile::parse_with`]). The object readers and [`decode::decode`], used directly,
//! do not track total work.
//!
//! Malformed data is meant to give an [`Error`] rather than a panic. `tests/fuzz.rs` checks
//! that over mutated files; it is evidence, not proof.

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

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

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The README's examples, compiled as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

/// Everything that can go wrong reading a file.
///
/// Strings that came from a file are quoted (`{:?}`) when displayed. Each message is complete: an underlying cause is part of it rather
/// than a separate [`std::error::Error::source`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The data ended before a read finished. The offset counts from the start of the
    /// structure being read (an object, a header, a block table), not the file.
    #[error("unexpected end of data at byte {0}")]
    Truncated(usize),
    /// A length or count that cannot fit in the bytes that follow it. The offset counts as
    /// for [`Error::Truncated`].
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
    #[error("{}unsupported texture format {format}", texture_label(texture.as_deref()))]
    #[non_exhaustive]
    UnsupportedTextureFormat {
        /// The texture's name, when known.
        texture: Option<String>,
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
    #[error("sprite atlas {atlas} could not be read: {error}")]
    #[non_exhaustive]
    AtlasUnreadable {
        /// The atlas's path ID.
        atlas: i64,
        /// Why; shared by every sprite in the atlas.
        error: Arc<Self>,
    },
    /// A texture stored with no pixels, as dynamic font textures are; it is filled at run
    /// time.
    #[error("texture {0:?} is empty (0x0)")]
    EmptyTexture(String),
    /// Data that contradicts itself, or a layout this crate misread.
    #[error("invalid file: {0}")]
    Invalid(String),
    /// Reading a file from disk failed.
    #[error("{:?}: {error}", path.display().to_string())]
    #[non_exhaustive]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        error: std::io::Error,
    },
}

fn texture_label(name: Option<&str>) -> String {
    name.map_or_else(String::new, |n| format!("texture {n:?}: "))
}

impl Error {
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
            Self::TotalWork => "total pixel work",
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
    /// Most objects in one serialized file. Default 4,194,304.
    pub max_objects: u64,
    /// Most pixels in one texture this crate will decode. Default 16384 x 16384, Unity's
    /// own maximum. Decoding one that size peaks at 1-2 GiB, exporting a sprite from it at 2-3.
    pub max_texture_pixels: u64,
    /// Most triangles in one sprite's mesh. Default 65,536.
    pub max_sprite_triangles: u64,
    /// Most triangles across the sprites one [`Assets::sprites`] call returns. Default
    /// 4,194,304.
    pub max_total_triangles: u64,
    /// Most pixel tests spent masking one tight-packed sprite. Default 2^29, enough for a
    /// two-triangle mesh over a 16384 x 16384 sprite.
    pub max_mask_work: u64,
    /// Most pixels decoded, cut and masked, all told, across everything opened from one file
    /// or bundle. Default 2^34.
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

/// Work done and stream ranges read, shared by everything opened from one file or bundle, so
/// opening a bundle's files one by one does not multiply the budget.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    work: AtomicU64,
    /// Stream file or entry -> start -> (end, the texture that read it).
    streams: Mutex<HashMap<String, Claims>>,
}

/// One stream's claimed ranges: start -> (end, owner). The ranges never overlap.
type Claims = BTreeMap<u64, (u64, String)>;

/// Work reserved against [`Limits::max_total_work`], given back when dropped unless kept:
/// work that fails before it is done costs nothing.
pub(crate) struct Reservation<'a> {
    shared: &'a Shared,
    amount: u64,
    keep: bool,
}

impl Reservation<'_> {
    pub fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.keep {
            self.shared.work.fetch_sub(self.amount, Ordering::Relaxed);
        }
    }
}

impl Shared {
    /// Reserve `amount` of work, or refuse without taking any.
    pub fn reserve(&self, amount: u64, limit: u64) -> Result<Reservation<'_>> {
        self.work
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |w| {
                w.checked_add(amount).filter(|&t| t <= limit)
            })
            .map_err(|w| Error::LimitExceeded {
                kind: LimitKind::TotalWork,
                value: w.saturating_add(amount),
                limit,
            })?;
        Ok(Reservation {
            shared: self,
            amount,
            keep: false,
        })
    }

    pub fn work(&self) -> u64 {
        self.work.load(Ordering::Relaxed)
    }

    /// Record that `owner` reads `start..end` of `stream`, refusing a range another texture
    /// already read any part of: one blob decoded many times over.
    #[allow(clippy::significant_drop_tightening)] // the check and the insert are one step
    pub fn claim_stream(&self, stream: &str, start: u64, end: u64, owner: &str) -> Result<()> {
        let mut streams = self
            .streams
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ranges = streams.entry(stream.to_string()).or_default();
        if let Some((&s, (e, who))) = ranges.range(..end).next_back() {
            let same = s == start && *e == end && who == owner;
            if *e > start && !same {
                return Err(Error::Invalid(format!(
                    "{owner} streams bytes {start}..{end} of {stream:?}, which {who} already \
                     read ({s}..{e})"
                )));
            }
            if same {
                return Ok(());
            }
        }
        ranges.insert(start, (end, owner.to_string()));
        Ok(())
    }
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
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
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
    /// betas, betas before every released build of the same numbers.
    pub fn at_least(&self, numbers: [u32; 3], kind: char, build: u32) -> bool {
        let rank = |k: char| match k {
            'a' => 0,
            'b' => 1,
            _ => 2,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            texture: Some("t".into()),
            format: 25,
        };
        assert_eq!(
            e.to_string(),
            "texture \"t\": unsupported texture format 25"
        );
        let e = Error::UnsupportedTextureFormat {
            texture: None,
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
