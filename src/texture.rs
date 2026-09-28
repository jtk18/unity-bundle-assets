//! `Texture2D`: dimensions, pixel format, and where the pixels live (inline, or in a `.resS`
//! stream file next to the serialized file or inside its bundle).

use crate::bundle::Bundle;
use crate::decode;
use crate::reader::Reader;
use crate::serialized::{ObjectInfo, SerializedFile};
use crate::{check_release, Error, Result};

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

/// Largest width or height Unity writes.
const MAX_DIMENSION: i32 = 16384;

/// Unity `BuildTarget`s whose textures may be tiled or swizzled for the console's GPU:
/// PS3, Xbox 360, PS Vita, PS4, Xbox One, 3DS, Wii U, Switch, the Xbox `GameCore` targets, PS5.
const CONSOLES: [i32; 11] = [10, 11, 30, 31, 33, 35, 36, 38, 42, 43, 44];

/// Whether files built for `target_platform` may hold textures in a console GPU's tiled
/// layout, which this crate does not undo.
#[must_use]
pub fn is_console_platform(target_platform: i32) -> bool {
    CONSOLES.contains(&target_platform)
}

/// Where a texture's pixels live when they are not stored in the object.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A `Texture2D` object: dimensions, pixel format, and where the pixels are. Borrows its
/// inline pixels from the file it was read from.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Texture2D<'a> {
    /// `m_Name`.
    pub name: String,
    /// Width in pixels; 0 for a texture stored empty.
    pub width: u32,
    /// Height in pixels; 0 for a texture stored empty.
    pub height: u32,
    /// Unity's `TextureFormat` value; see [`crate::decode::format`].
    pub format: i32,
    /// Number of mip levels stored.
    pub mip_count: u32,
    /// `m_PlatformBlob`: platform-specific layout data (Unity 2020.2 on).
    pub platform_blob: &'a [u8],
    /// Pixels stored inside the object, every mip level, when not streamed.
    pub image_data: &'a [u8],
    /// Where the pixels are, when streamed.
    pub stream: Option<StreamingInfo>,
}

impl std::fmt::Debug for Texture2D<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Texture2D")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.format)
            .field("mip_count", &self.mip_count)
            .field("platform_blob", &self.platform_blob.len())
            .field("image_data", &self.image_data.len())
            .field("stream", &self.stream)
            .finish()
    }
}

impl<'a> Texture2D<'a> {
    /// Read a `Texture2D` object, Unity 5.5 through 6000.4 (final and beta builds).
    ///
    /// Fields are gated by engine release, checked against the per-release type trees that
    /// ship with `UnityPy` (see NOTICE). Where a boundary release's builds differ, the
    /// difference falls inside padding and reads the same.
    ///
    /// # Errors
    ///
    /// For an engine release outside that range, or an object that does not fit the layout.
    pub fn read(file: &'a SerializedFile, object: &ObjectInfo) -> Result<Self> {
        check_release(file.unity_version(), "Texture2D", [5, 5, 0])?;
        let mut r = file.reader(object)?;
        read_fields(&mut r, file.unity_version_numbers()).map_err(|e| match e {
            Error::Truncated(_) | Error::BadLength { .. } => Error::Invalid(format!(
                "Texture2D {} from Unity {:?} does not fit the layout this crate knows ({e})",
                object.path_id(),
                file.unity_version()
            )),
            e => e,
        })
    }

    /// Bytes to read for the first mip level, refusing formats this crate cannot decode
    /// before anything is read.
    fn wanted(&self, stored: usize) -> Result<usize> {
        decode::mip0_size(self.format, self.width, self.height)
            .map(|need| need.min(stored))
            .ok_or_else(|| Error::UnsupportedTextureFormat {
                texture: Some(self.name.clone()),
                format: self.format,
            })
    }

    /// The first mip level's pixel data. Streamed data is read from `dir`, the folder holding
    /// the serialized file, and only from a `.resS` or `.resource` file directly inside it that
    /// is a regular file and not a symbolic link.
    ///
    /// # Errors
    ///
    /// For a format this crate does not decode, a stream path outside those rules, a file that
    /// cannot be read, or a stream running past the end of its file.
    pub fn data(&self, dir: &Path) -> Result<Cow<'a, [u8]>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(stream) = &self.stream else {
            let want = self.wanted(self.image_data.len())?;
            return Ok(Cow::Borrowed(&self.image_data[..want]));
        };
        if stream.path.starts_with("archive:") {
            return Err(Error::Unsupported(format!(
                "texture {:?} streams from an asset bundle ({:?}); use Texture2D::data_in",
                self.name, stream.path
            )));
        }
        let want = self.wanted(stream.size as usize)?;
        let path = stream_file(dir, &stream.path)?;
        let (mut file, len) = crate::file::open_regular(&path, false)?;
        let end = stream.offset.checked_add(u64::from(stream.size));
        if end.is_none_or(|end| end > len) {
            return Err(Error::Invalid(format!(
                "texture {:?} streams {} bytes at {} from {:?}, which holds {len}",
                self.name,
                stream.size,
                stream.offset,
                path.display().to_string()
            )));
        }
        file.seek(SeekFrom::Start(stream.offset))
            .map_err(Error::io(&path))?;
        let mut data = Vec::with_capacity(want);
        file.take(want as u64)
            .read_to_end(&mut data)
            .map_err(Error::io(&path))?;
        Ok(Cow::Owned(data))
    }

    /// The first mip level's pixel data, for a texture read from a bundle: streamed data is
    /// read from the bundle's own `.resS` entry.
    ///
    /// # Errors
    ///
    /// For a format this crate does not decode, a stream entry the bundle does not hold, or a
    /// stream running past the end of its entry.
    pub fn data_in<'b>(&'b self, bundle: &'b Bundle) -> Result<Cow<'b, [u8]>> {
        let Some(stream) = &self.stream else {
            let want = self.wanted(self.image_data.len())?;
            return Ok(Cow::Borrowed(&self.image_data[..want]));
        };
        let want = self.wanted(stream.size as usize)?;
        let entry = bundle.entry(&stream.path).ok_or_else(|| {
            Error::NotFound(format!(
                "stream {:?} of texture {:?} in this bundle",
                stream.path, self.name
            ))
        })?;
        let bytes = bundle.bytes(entry).unwrap_or_default();
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
                    "texture {:?} streams past the end of {:?}",
                    self.name,
                    entry.path()
                ))
            })
    }
}

/// `name` as a stream file directly inside `dir`: one plain path component ending in `.resS`
/// or `.resource`, the names Unity writes. Anything else is refused.
fn stream_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let refuse = || {
        Error::Unsupported(format!(
            "stream path {name:?} is not a .resS or .resource file name beside the asset file"
        ))
    };
    let mut parts = Path::new(name).components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(file)), None) => {
            let file = file.to_str().ok_or_else(refuse)?;
            let (stem, ext) = file.rsplit_once('.').ok_or_else(refuse)?;
            if stem.is_empty() || !(ext == "resS" || ext == "resource") {
                return Err(refuse());
            }
            Ok(dir.join(file))
        }
        _ => Err(refuse()),
    }
}

fn read_fields<'a>(r: &mut Reader<'a>, v: [u32; 3]) -> Result<Texture2D<'a>> {
    let at_least = |ma: u32, mi: u32, pa: u32| v >= [ma, mi, pa];
    let name = r.aligned_string()?;
    // m_ForcedFallbackFormat and m_DownscaleFallback: 2017.3 up to 2023.2.
    if at_least(2017, 3, 0) && !at_least(2023, 2, 0) {
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
    if at_least(2019, 4, 9) {
        r.bool()?; // m_IsPreProcessed
    }
    if at_least(2019, 3, 0) {
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
        r.byte_array()?
    } else {
        &[]
    };
    let image_data = r.byte_array()?;
    // m_StreamData is always present; it names a stream only when there are no inline pixels.
    let offset = if at_least(2020, 1, 0) {
        r.u64()?
    } else {
        u64::from(r.u32()?)
    };
    let size = r.u32()?;
    let path = r.aligned_string()?;
    let stream =
        Some(StreamingInfo { offset, size, path }).filter(|s| image_data.is_empty() && s.size > 0);
    // The last field ends the object. Bytes left over mean the layout was misread.
    if r.remaining() != 0 {
        return Err(Error::Invalid(format!(
            "texture {name:?} has {} bytes after its last field; the layout is probably \
             misread",
            r.remaining()
        )));
    }
    let empty = width == 0 && height == 0 && image_data.is_empty() && stream.is_none();
    let in_range = |d: i32| (1..=MAX_DIMENSION).contains(&d);
    if !(empty || in_range(width) && in_range(height)) {
        return Err(Error::Invalid(format!(
            "texture {name:?} is {width}x{height}; the layout is probably misread"
        )));
    }
    Ok(Texture2D {
        name,
        width: width.unsigned_abs(),
        height: height.unsigned_abs(),
        format,
        mip_count: mip_count.unsigned_abs(),
        platform_blob,
        image_data,
        stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_file_names() {
        let dir = Path::new("/d");
        assert_eq!(
            stream_file(dir, "a.assets.resS").unwrap(),
            dir.join("a.assets.resS")
        );
        assert!(stream_file(dir, "level0.resource").is_ok());
        for bad in [
            ".netrc",
            "a.txt",
            ".resS",
            "x/a.resS",
            "../a.resS",
            "/a.resS",
            "",
            ".",
        ] {
            assert!(stream_file(dir, bad).is_err(), "{bad}");
        }
    }
}
