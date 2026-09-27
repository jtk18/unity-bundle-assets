//! Read Unity serialized asset files and export their sprites, in pure Rust.

pub mod decode;
pub mod export;
pub mod reader;
pub mod serialized;
pub mod sprite;
pub mod texture;

pub use export::{Assets, Image};
pub use serialized::{class, ObjectInfo, SerializedFile};
pub use sprite::{Placement, Sprite, SpriteAtlas};
pub use texture::Texture2D;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data at byte {0}")]
    Truncated(usize),
    #[error("bad length {len} at byte {at}")]
    BadLength { at: usize, len: i64 },
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("invalid file: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
