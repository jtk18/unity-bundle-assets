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
//! Files are treated as hostile. Counts and lengths are checked against the bytes present
//! before they size an allocation, sizes that the data alone cannot bound (decompressed
//! bundles, decoded pixels, sprite meshes) are held to [`Limits`], and a texture's streamed
//! pixels are only ever read from a plain file beside the asset file or from its own bundle.
//! Malformed data is meant to give an [`Error`] rather than a panic; the crate is fuzzed for
//! that, which is evidence, not proof.

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations)]

mod bundle;
pub mod decode;
mod export;
mod reader;
mod serialized;
mod sprite;
mod texture;

pub use bundle::{is_bundle, Bundle, Entry};
pub use export::{Assets, Image, SkippedSprite, SpriteList, TextureInfo};
pub use serialized::{class, External, ObjectInfo, SerializedFile, SerializedType};
pub use sprite::{PPtr, Placement, Rect, RenderDataKey, Rotation, Settings, Sprite, SpriteAtlas};
pub use texture::{StreamingInfo, Texture2D};

/// Everything that can go wrong reading a file.
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
    #[error("texture {texture:?}: unsupported texture format {format}")]
    UnsupportedTextureFormat {
        /// The texture's name, when known.
        texture: String,
        /// Unity's `TextureFormat` value.
        format: i32,
    },
    /// An encrypted asset bundle.
    #[error("encrypted asset bundle")]
    Encrypted,
    /// A size over one of the [`Limits`] in force.
    #[error("{what} is {value}, over the limit of {limit}")]
    LimitExceeded {
        /// What was measured.
        what: &'static str,
        /// Its size.
        value: u64,
        /// The limit.
        limit: u64,
    },
    /// No object or entry by that ID or path.
    #[error("not found: {0}")]
    NotFound(String),
    /// A texture stored with no pixels, as dynamic font textures are; it is filled at run
    /// time.
    #[error("texture {0:?} is empty (0x0)")]
    EmptyTexture(String),
    /// Data that contradicts itself, or a layout this crate misread.
    #[error("invalid file: {0}")]
    Invalid(String),
    /// Reading a file from disk failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Ceilings on work and memory that the data alone cannot bound. The defaults sit well above
/// what real games use; lower them for batch tools that open files from strangers.
///
/// ```
/// let mut limits = unity_bundle_assets::Limits::default();
/// limits.max_decompressed = 256 << 20;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Largest file [`Assets::open`] and friends will read, in bytes. Default 2 GiB.
    pub max_file_size: u64,
    /// Largest decompressed size of a bundle, directory included, in bytes. Default 1 GiB.
    pub max_decompressed: u64,
    /// Most pixels in a texture this crate will decode. Default 8192 x 8192.
    pub max_texture_pixels: u64,
    /// Most triangles in one sprite's mesh. Default 65,536.
    pub max_sprite_triangles: usize,
    /// Most triangles across the sprites one [`Assets::sprites`] call returns. Default
    /// 4,194,304.
    pub max_total_triangles: usize,
    /// Most pixel tests spent masking one tight-packed sprite. Default 2^28.
    pub max_mask_work: u64,
}

impl Limits {
    /// The defaults, as a constant.
    pub const DEFAULT: Limits = Limits {
        max_file_size: 2 << 30,
        max_decompressed: 1 << 30,
        max_texture_pixels: 8192 * 8192,
        max_sprite_triangles: 1 << 16,
        max_total_triangles: 1 << 22,
        max_mask_work: 1 << 28,
    };
}

impl Default for Limits {
    fn default() -> Limits {
        Limits::DEFAULT
    }
}

/// An engine version string as numbers: `2022.3.62f3` is `[2022, 3, 62]`. Anything that does
/// not parse is `[0, 0, 0]`.
pub(crate) fn version_numbers(version: &str) -> [u32; 3] {
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(
        version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty()),
    ) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

/// The newest engine release whose layouts this crate was checked against. Files from later
/// releases are refused rather than guessed at.
pub(crate) const NEWEST_KNOWN: [u32; 2] = [6000, 5];

/// Refuse object layouts from releases outside `[oldest, NEWEST_KNOWN]`.
pub(crate) fn check_release(
    version: &str,
    numbers: [u32; 3],
    what: &str,
    oldest: [u32; 3],
) -> Result<()> {
    if numbers < oldest {
        return Err(Error::Unsupported(format!(
            "{what} from Unity {version} (needs {}.{} or later)",
            oldest[0], oldest[1]
        )));
    }
    if [numbers[0], numbers[1]] > NEWEST_KNOWN {
        return Err(Error::Unsupported(format!(
            "{what} from Unity {version}, newer than the layouts this crate knows (up to {}.{})",
            NEWEST_KNOWN[0], NEWEST_KNOWN[1]
        )));
    }
    Ok(())
}
