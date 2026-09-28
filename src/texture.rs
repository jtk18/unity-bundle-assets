//! `Texture2D`: dimensions, pixel format, and where the pixels live (inline, or in a `.resS`
//! stream file next to the serialized file or inside its bundle).

use crate::bundle::Bundle;
use crate::decode;
use crate::reader::Reader;
use crate::serialized::{class, ObjectInfo, SerializedFile};
use crate::{check_release, quoted, quoted_path, Error, Result, StreamKey, Version};

use std::borrow::Cow;
use std::path::{Path, PathBuf};

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
    /// Read a `Texture2D` object, Unity 5.5 through 6000.4: final, patch, China and beta
    /// builds (6000.4 finals are read with its betas' layout, which is all that was checked).
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
        let mut r = file.reader_for(object, class::TEXTURE_2D)?;
        read_fields(&mut r, Version::parse(file.unity_version())).map_err(|e| match e {
            Error::Truncated(_) | Error::BadLength { .. } => Error::Invalid(format!(
                "Texture2D {} from Unity {} does not fit the layout this crate knows ({e})",
                object.path_id(),
                quoted(file.unity_version())
            )),
            e => e,
        })
    }

    /// Bytes to read for the first mip level, refusing formats this crate cannot decode and
    /// data too short to hold the level before anything is read.
    fn wanted(&self, stored: usize) -> Result<usize> {
        let need = decode::mip0_size(self.format, self.width, self.height).ok_or_else(|| {
            Error::UnsupportedTextureFormat {
                name: Some(self.name.clone()),
                format: self.format,
            }
        })?;
        if stored < need {
            return Err(Error::Invalid(format!(
                "texture {} holds {stored} bytes of pixels; its first mip level needs {need}",
                quoted(&self.name)
            )));
        }
        Ok(need)
    }

    /// The first mip level's pixel data. Streamed data is read from `dir`, the folder holding
    /// the serialized file, and only from a `.resS` or `.resource` file directly inside it: a
    /// regular file, not a symbolic link, with no other hard links (checked on Unix only), and
    /// not named like a Windows device or an alternate data stream.
    ///
    /// This does no bookkeeping across textures: two textures naming the same bytes both read
    /// them. [`crate::Assets::decode_texture`] refuses that and counts the work.
    ///
    /// # Errors
    ///
    /// For a format this crate does not decode, a stream path outside those rules, a file that
    /// cannot be read, or pixel data too short for the texture.
    pub fn data(&self, dir: &Path) -> Result<Cow<'a, [u8]>> {
        self.data_claimed(dir, &mut |_, _, _| Ok(()))
    }

    /// [`Texture2D::data`], calling `claim` with the stream range once it is known to be valid
    /// and before any of it is read.
    pub(crate) fn data_claimed(&self, dir: &Path, claim: Claim<'_>) -> Result<Cow<'a, [u8]>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(stream) = &self.stream else {
            let want = self.wanted(self.image_data.len())?;
            return Ok(Cow::Borrowed(&self.image_data[..want]));
        };
        if stream.path.starts_with("archive:") {
            return Err(Error::Unsupported(format!(
                "texture {} streams from an asset bundle ({}); use Texture2D::data_in",
                quoted(&self.name),
                quoted(&stream.path)
            )));
        }
        let want = self.wanted(stream.size as usize)?;
        let path = stream_file(dir, &stream.path)?;
        // Stream files are read by range, not whole, so `max_file_size` does not apply: real
        // ones pass 2 GiB (a 2022.3 build's `sharedassets0.assets.resS` is 2.4 GB).
        let (mut file, len) = crate::file::open_regular(&path, crate::file::Chosen::ByData)?;
        let end = stream.offset.checked_add(u64::from(stream.size));
        let Some(end) = end.filter(|&end| end <= len) else {
            return Err(Error::Invalid(format!(
                "texture {} streams {} bytes at {} from {}, which holds {len}",
                quoted(&self.name),
                stream.size,
                stream.offset,
                quoted_path(&path.display().to_string())
            )));
        };
        let id = crate::file::identity(&file, &path)?;
        claim(StreamKey::File(id), stream.offset, end)?;
        file.seek(SeekFrom::Start(stream.offset))
            .map_err(Error::io(&path))?;
        let mut data = Vec::new();
        data.try_reserve_exact(want)
            .map_err(|_| Error::OutOfMemory { bytes: want as u64 })?;
        file.take(want as u64)
            .read_to_end(&mut data)
            .map_err(Error::io(&path))?;
        if data.len() != want {
            return Err(Error::Invalid(format!(
                "{} ended while texture {} was being read",
                quoted_path(&path.display().to_string()),
                quoted(&self.name)
            )));
        }
        Ok(Cow::Owned(data))
    }

    /// The first mip level's pixel data, for a texture read from a bundle: streamed data is
    /// read from one of the bundle's stream entries (never a serialized file).
    ///
    /// # Errors
    ///
    /// For a format this crate does not decode, a stream entry the bundle does not hold or
    /// that is a serialized file, or pixel data too short for the texture.
    pub fn data_in<'b>(&self, bundle: &'b Bundle) -> Result<Cow<'b, [u8]>>
    where
        'a: 'b,
    {
        self.data_in_claimed(bundle, &mut |_, _, _| Ok(()))
    }

    /// [`Texture2D::data_in`], calling `claim` with the stream's range in the bundle's data
    /// once it is known to be valid.
    pub(crate) fn data_in_claimed<'b>(
        &self,
        bundle: &'b Bundle,
        claim: Claim<'_>,
    ) -> Result<Cow<'b, [u8]>>
    where
        'a: 'b,
    {
        let Some(stream) = &self.stream else {
            let want = self.wanted(self.image_data.len())?;
            return Ok(Cow::Borrowed(&self.image_data[..want]));
        };
        let want = self.wanted(stream.size as usize)?;
        let entry = bundle.entry(&stream.path).ok_or_else(|| {
            Error::NotFound(format!(
                "stream {} of texture {} in this bundle",
                quoted(&stream.path),
                quoted(&self.name)
            ))
        })?;
        if entry.is_serialized() {
            return Err(Error::Invalid(format!(
                "texture {} streams from {}, a serialized file",
                quoted(&self.name),
                quoted(entry.path())
            )));
        }
        let bytes = bundle.bytes(entry).unwrap_or_default();
        let start = usize::try_from(stream.offset)
            .ok()
            .filter(|&start| {
                start
                    .checked_add(stream.size as usize)
                    .is_some_and(|end| end <= bytes.len())
            })
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "texture {} streams past the end of {}",
                    quoted(&self.name),
                    quoted(entry.path())
                ))
            })?;
        // Claimed by position in the bundle's data, so every path that resolves to this entry
        // claims the same bytes.
        let at = (entry.offset + start) as u64;
        claim(StreamKey::Bundle, at, at + u64::from(stream.size))?;
        Ok(Cow::Borrowed(&bytes[start..start + want]))
    }
}

/// Called with a stream range, `(what, start, end)`, before it is read.
pub(crate) type Claim<'c> = &'c mut dyn FnMut(StreamKey, u64, u64) -> Result<()>;

/// `name` as a stream file directly inside `dir`: one plain file name ending in `.resS` or
/// `.resource`, the names Unity writes. Anything else is refused: path separators, `.` and
/// `..`, Windows alternate data streams (`:`) and device names, which Windows matches
/// ignoring case, trailing spaces and dots, and any extension (`CON .resS`, `com1.resS`).
fn stream_file(dir: &Path, name: &str) -> Result<PathBuf> {
    let refuse = || {
        Error::Unsupported(format!(
            "stream path {name} is not a .resS or .resource file name beside the asset file",
            name = quoted(name)
        ))
    };
    // Unity writes these names in ASCII (`CAB-<hash>.resS`, `sharedassets0.assets.resS`).
    // Anything else could name one file two ways on a system that folds case beyond ASCII,
    // which the claims (by lower-cased name there) would not see.
    if !name.is_ascii() || name.contains(['/', '\\', ':', '\0']) {
        return Err(refuse());
    }
    let (stem, ext) = name.rsplit_once('.').ok_or_else(refuse)?;
    if stem.is_empty() || !(ext == "resS" || ext == "resource") {
        return Err(refuse());
    }
    if is_windows_device(name) {
        return Err(refuse());
    }
    Ok(dir.join(name))
}

/// Whether Windows would open `name` as a device: its part before the first dot, less
/// trailing spaces and dots, is a reserved name.
fn is_windows_device(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or("");
    let base = base.trim_end_matches([' ', '.']).to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str()) {
        return true;
    }
    let mut chars = base.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    // COM0 and LPT0 too: Windows' naming rules list them, whatever its device layer does.
    // (Superscript digits, which Windows also matches, are refused as non-ASCII.)
    (prefix == "COM" || prefix == "LPT")
        && chars.next().is_none()
        && digit.is_some_and(|d| d.is_ascii_digit())
}

fn read_fields<'a>(r: &mut Reader<'a>, version: Version) -> Result<Texture2D<'a>> {
    let v = version.numbers;
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
    if at_least(2020, 1, 0) && r.i32()? != 0 {
        // The stored pixels would start at a smaller mip than the width and height describe.
        return Err(Error::Unsupported(format!(
            "texture {name} has stripped mip levels",
            name = quoted(&name)
        )));
    }
    let format = r.i32()?;
    let mip_count = r.i32()?;
    r.bool()?; // m_IsReadable
    if at_least(2019, 4, 9) {
        r.bool()?; // m_IsPreProcessed
    }
    if version.at_least([2019, 3, 0], 'f', 5) {
        r.bool()?; // m_IgnoreMasterTextureLimit / m_IgnoreMipmapLimit
    }
    // The limit group name arrived in 2022.2.0b3; b1 and b2 lack it.
    if version.at_least([2022, 2, 0], 'b', 3) {
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
    let path = r.aligned_path()?;
    let stream =
        Some(StreamingInfo { offset, size, path }).filter(|s| image_data.is_empty() && s.size > 0);
    // The last field ends the object. Bytes left over mean the layout was misread.
    r.check_end(|| format!("texture {}", quoted(&name)))?;
    let Ok(mip_count) = u32::try_from(mip_count) else {
        return Err(Error::Invalid(format!(
            "texture {name} has {mip_count} mip levels",
            name = quoted(&name)
        )));
    };
    let empty = width == 0 && height == 0 && image_data.is_empty() && stream.is_none();
    let in_range = |d: i32| (1..=MAX_DIMENSION).contains(&d);
    if !(empty || in_range(width) && in_range(height)) {
        return Err(Error::Invalid(format!(
            "texture {name} is {width}x{height}; the layout is probably misread",
            name = quoted(&name)
        )));
    }
    Ok(Texture2D {
        name,
        // Both are within 0..=16384 here.
        width: width as u32,
        height: height as u32,
        format,
        mip_count,
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
