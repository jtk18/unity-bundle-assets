//! Read Unity serialized asset files and asset bundles, and export their sprites and
//! textures as RGBA images, in pure Rust.
//!
//! [`Assets`] is the entry point. It opens a serialized file (`*.assets`, `level*`) or an
//! asset bundle holding one, lists its sprites and textures, and decodes them:
//!
//! ```no_run
//! # fn main() -> Result<(), unity_bundle_assets::Error> {
//! use unity_bundle_assets::Assets;
//!
//! // Sprites, cut out of their textures or atlases.
//! let mut assets = Assets::open("Game_Data/sharedassets0.assets".as_ref())?;
//! for sprite in assets.sprites(|name| name.starts_with("Icon_"))? {
//!     let image = assets.export(&sprite)?; // RGBA8, top row first
//! }
//!
//! // Whole textures, from a bundle.
//! let assets = Assets::open("assetbundles/characters".as_ref())?;
//! for texture in assets.textures(|_| true) {
//!     let image = assets.decode_texture(texture.path_id)?; // RGBA8, bottom row first
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The lower layers are public too: [`Bundle`] for the container, [`SerializedFile`] for the
//! object table, [`Texture2D`], [`Sprite`] and [`SpriteAtlas`] for single objects, and
//! [`decode`] for pixel formats.
//!
//! Every input is treated as untrusted: lengths are checked against the bytes present
//! before anything is allocated, and malformed data gives an [`Error`], never a panic.

#![warn(missing_docs)]

mod bundle;
pub mod decode;
mod export;
mod reader;
mod serialized;
mod sprite;
mod texture;

pub use bundle::{is_bundle, Bundle, Entry};
pub use export::{Assets, Image, TextureInfo};
pub use serialized::{class, External, ObjectInfo, SerializedFile, SerializedType};
pub use sprite::{PPtr, Placement, Rect, Rotation, Settings, Sprite, SpriteAtlas};
pub use texture::{StreamingInfo, Texture2D};

/// Everything that can go wrong reading a file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The data ended before a read finished; the value is the byte offset reached.
    #[error("unexpected end of data at byte {0}")]
    Truncated(usize),
    /// A length or count that cannot fit in the bytes that follow it.
    #[error("bad length {len} at byte {at}")]
    BadLength {
        /// Byte offset of the length field.
        at: usize,
        /// The length it claimed.
        len: i64,
    },
    /// Valid data this crate does not read yet: a format version, container, compression or
    /// pixel format. The message names it.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Data that contradicts itself, such as an object running past the end of its file.
    #[error("invalid file: {0}")]
    Invalid(String),
    /// Reading a file from disk failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `Result` with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;
