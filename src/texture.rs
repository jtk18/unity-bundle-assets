//! `Texture2D`: dimensions, pixel format, and where the pixels live (inline, or in a `.resS`
//! stream file next to the serialized file or inside its bundle).

use crate::bundle::Bundle;
use crate::decode;
use crate::reader::Reader;
use crate::serialized::{ObjectInfo, SerializedFile};
use crate::{check_release, Error, Result};

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

/// Unity's `BuildTarget` for Nintendo Switch, whose textures may be swizzled.
const SWITCH: i32 = 38;
/// Largest width or height Unity writes.
const MAX_DIMENSION: i32 = 16384;

/// Where a texture's pixels live when they are not stored in the object.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StreamingInfo {
    /// Byte offset in the stream file.
    pub offset: u64,
    /// Byte count.
    pub size: u32,
    /// The stream file: a `.resS` beside the serialized file, or an `archive:/` path into
    /// the bundle holding it.
    pub path: String,
}

/// A `Texture2D` object: dimensions, pixel format, and where the pixels are.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Texture2D {
    /// `m_Name`.
    pub name: String,
    /// Width in pixels; 0 for a texture stored empty.
    pub width: u32,
    /// Height in pixels; 0 for a texture stored empty.
    pub height: u32,
    /// Unity's `TextureFormat` value; see [`crate::decode::format`].
    pub format: i32,
    /// Number of mip levels stored.
    pub mip_count: i32,
    /// `m_PlatformBlob`: platform-specific layout data (Unity 2020.2 on).
    pub platform_blob: Vec<u8>,
    /// Pixels stored inside the object, when not streamed.
    pub image_data: Vec<u8>,
    /// Where the pixels are, when streamed.
    pub stream: Option<StreamingInfo>,
}

impl Texture2D {
    /// Read a `Texture2D` object, Unity 5.5 through 6000.5.
    ///
    /// Fields are gated by engine release, checked against the per-release type trees that
    /// ship with UnityPy (see NOTICE). Within a release, alpha and beta builds may differ at a
    /// boundary; where they do, the difference falls inside padding and reads the same.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Texture2D> {
        let v = file.unity_version_numbers();
        check_release(file.unity_version(), v, "Texture2D", [5, 5, 0])?;
        let mut r = file.reader(object)?;
        read_fields(&mut r, v).map_err(|e| match e {
            Error::Truncated(_) | Error::BadLength { .. } => Error::Invalid(format!(
                "Texture2D {} from Unity {} does not fit the layout this crate knows ({e})",
                object.path_id(),
                file.unity_version()
            )),
            e => e,
        })
    }

    /// Whether the pixels are swizzled for Nintendo Switch, which this crate does not undo.
    pub fn is_switch_swizzled(&self, target_platform: i32) -> bool {
        target_platform == SWITCH
            && self
                .platform_blob
                .get(8..12)
                .is_some_and(|b| u32::from_le_bytes(b.try_into().unwrap()) > 0)
    }

    /// Bytes to read for the first mip level: the stored size, trimmed to what decoding
    /// needs when the format is known.
    fn wanted(&self, stored: usize) -> usize {
        decode::mip0_size(self.format, self.width, self.height)
            .map_or(stored, |need| need.min(stored))
    }

    /// The pixel data, first mip level first. Streamed data is read from `dir`, the folder
    /// holding the serialized file, and only from a plain file directly inside it: a stream
    /// path with a directory, a root, `..` or a drive in it is refused, as is anything that
    /// is not a regular file.
    pub fn data(&self, dir: &Path) -> Result<Cow<'_, [u8]>> {
        let Some(stream) = &self.stream else {
            return Ok(Cow::Borrowed(&self.image_data));
        };
        if stream.path.starts_with("archive:") {
            return Err(Error::Unsupported(format!(
                "texture {} streams from an asset bundle ({}); use Texture2D::data_in",
                self.name, stream.path
            )));
        }
        let path = beside(dir, &stream.path)?;
        let not_regular = || {
            Error::Unsupported(format!(
                "texture {} streams from {}, which is not a regular file",
                self.name,
                path.display()
            ))
        };
        // Checked before opening: opening a FIFO blocks.
        if !std::fs::metadata(&path)?.is_file() {
            return Err(not_regular());
        }
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&path)?;
        let meta = f.metadata()?;
        if !meta.is_file() {
            return Err(not_regular());
        }
        let len = meta.len();
        let end = stream.offset.checked_add(stream.size as u64);
        if end.is_none_or(|end| end > len) {
            return Err(Error::Invalid(format!(
                "texture {} streams {} bytes at {} from {}, which holds {len}",
                self.name,
                stream.size,
                stream.offset,
                path.display()
            )));
        }
        let want = self.wanted(stream.size as usize);
        f.seek(SeekFrom::Start(stream.offset))?;
        let mut data = Vec::with_capacity(want);
        f.take(want as u64).read_to_end(&mut data)?;
        Ok(Cow::Owned(data))
    }

    /// The pixel data, first mip level first, for a texture read from a bundle: streamed data
    /// is read from the bundle's own `.resS` entry.
    pub fn data_in<'a>(&'a self, bundle: &'a Bundle) -> Result<Cow<'a, [u8]>> {
        let Some(stream) = &self.stream else {
            return Ok(Cow::Borrowed(&self.image_data));
        };
        let entry = bundle.entry(&stream.path).ok_or_else(|| {
            Error::NotFound(format!(
                "stream {} of texture {} in this bundle",
                stream.path, self.name
            ))
        })?;
        let bytes = bundle.bytes(entry).unwrap_or_default();
        let want = self.wanted(stream.size as usize);
        usize::try_from(stream.offset)
            .ok()
            .filter(|&start| {
                start
                    .checked_add(stream.size as usize)
                    .is_some_and(|end| end <= bytes.len())
            })
            .map(|start| Cow::Borrowed(&bytes[start..start + want]))
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "texture {} streams past the end of {}",
                    self.name,
                    entry.path()
                ))
            })
    }
}

/// `name` as a file directly inside `dir`, or an error if it is anything else.
fn beside(dir: &Path, name: &str) -> Result<PathBuf> {
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(file)), None) => Ok(dir.join(file)),
        _ => Err(Error::Unsupported(format!(
            "stream path {name:?} is not a file name beside the asset file"
        ))),
    }
}

fn read_fields(r: &mut Reader, v: [u32; 3]) -> Result<Texture2D> {
    let at_least = |ma: u32, mi: u32, pa: u32| v >= [ma, mi, pa];
    let name = r.aligned_string()?;
    // m_ForcedFallbackFormat and m_DownscaleFallback: 2017.3 up to 2023.2.
    let fallback = at_least(2017, 3, 0) && !at_least(2023, 2, 0);
    if fallback {
        r.i32()?; // m_ForcedFallbackFormat
        r.bool()?; // m_DownscaleFallback
    }
    if at_least(2020, 2, 0) {
        r.bool()?; // m_IsAlphaChannelOptional
    }
    r.align(4);
    let width = r.i32()?;
    let height = r.i32()?;
    r.i32()?; // m_CompleteImageSize
    if at_least(2020, 1, 0) {
        r.i32()?; // m_MipsStripped
    }
    let format = r.i32()?;
    let mip_count = r.i32()?;
    r.bool()?; // m_IsReadable
    if !at_least(5, 5, 1) {
        r.bool()?; // m_ReadAllowed
    }
    if at_least(2019, 4, 9) {
        r.bool()?; // m_IsPreProcessed
    }
    if at_least(2019, 3, 1) {
        r.bool()?; // m_IgnoreMasterTextureLimit / m_IgnoreMipmapLimit
    }
    if at_least(2022, 2, 0) {
        r.align(4);
        r.aligned_string()?; // m_MipmapLimitGroupName
    }
    if at_least(2018, 2, 0) {
        r.bool()?; // m_StreamingMipmaps
        r.align(4);
        r.i32()?; // m_StreamingMipmapsPriority
    }
    r.align(4);
    r.i32()?; // m_ImageCount
    r.i32()?; // m_TextureDimension
              // m_TextureSettings: filter, aniso, mip bias, then wrap U/V/W (one wrap mode before 2017)
    r.skip(if at_least(2017, 1, 0) { 24 } else { 16 })?;
    r.i32()?; // m_LightmapFormat
    r.i32()?; // m_ColorSpace
    let platform_blob = if at_least(2020, 2, 0) {
        r.byte_array()?.to_vec()
    } else {
        Vec::new()
    };
    let image_data = r.byte_array()?.to_vec();
    let stream = if image_data.is_empty() {
        let wide = at_least(2020, 1, 0);
        let offset = if wide { r.u64()? } else { r.u32()? as u64 };
        let size = r.u32()?;
        let path = r.aligned_string()?;
        Some(StreamingInfo { offset, size, path }).filter(|s| s.size > 0)
    } else {
        None
    };
    let empty = width == 0 && height == 0 && image_data.is_empty() && stream.is_none();
    let in_range = |d: i32| (1..=MAX_DIMENSION).contains(&d);
    if !empty && !(in_range(width) && in_range(height)) {
        return Err(Error::Invalid(format!(
            "texture {name} is {width}x{height}; the layout is probably misread"
        )));
    }
    Ok(Texture2D {
        name,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
        format,
        mip_count,
        platform_blob,
        image_data,
        stream,
    })
}
