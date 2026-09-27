//! `Texture2D`: dimensions, pixel format, and where the pixels live (inline, or in a `.resS`
//! stream file next to the serialized file).

use crate::bundle::Bundle;
use crate::reader::Reader;
use crate::serialized::{ObjectInfo, SerializedFile};
use crate::{Error, Result};

use std::path::Path;

#[derive(Debug, Clone)]
pub struct StreamingInfo {
    pub offset: u64,
    pub size: u32,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Texture2D {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Unity's `TextureFormat` value; see [`crate::decode::Format`].
    pub format: i32,
    pub mip_count: i32,
    /// Pixels stored inside the object, when not streamed.
    pub image_data: Vec<u8>,
    pub stream: Option<StreamingInfo>,
}

impl Texture2D {
    /// Field gates are by exact engine version, down to the patch where a field appeared;
    /// they were read off the type trees UnityPy ships for each release (see NOTICE).
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Texture2D> {
        let v = file.unity_version_numbers();
        let at_least = |ma: u32, mi: u32, pa: u32| v >= [ma, mi, pa];
        if !at_least(5, 5, 0) {
            return Err(Error::Unsupported(format!(
                "Texture2D from Unity {} (needs 5.5 or later)",
                file.unity_version
            )));
        }
        let mut r = file.reader(object);
        let name = r.aligned_string()?;
        if at_least(2017, 3, 0) {
            r.i32()?; // m_ForcedFallbackFormat
            r.bool()?; // m_DownscaleFallback
            if at_least(2020, 2, 0) {
                r.bool()?; // m_IsAlphaChannelOptional
            }
            r.align(4);
        }
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
        if at_least(2020, 2, 0) {
            r.byte_array()?; // m_PlatformBlob
        }
        let image_data = r.byte_array()?.to_vec();
        let stream = if image_data.is_empty() {
            Some(read_streaming_info(&mut r, at_least(2020, 1, 0))?)
        } else {
            None
        };
        if width == 0
            && height == 0
            && image_data.is_empty()
            && stream.as_ref().is_none_or(|s| s.size == 0)
        {
            // Dynamic font textures are stored empty and filled in at run time.
            return Err(Error::Invalid(format!("texture {name} is empty (0x0)")));
        }
        if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
            return Err(Error::Invalid(format!(
                "texture {name} is {width}x{height}; the layout for Unity {} is probably wrong",
                file.unity_version
            )));
        }
        Ok(Texture2D {
            name,
            width: width as u32,
            height: height as u32,
            format,
            mip_count,
            image_data,
            stream: stream.filter(|s| s.size > 0),
        })
    }

    /// The raw pixel data of every mip level. Streamed data is read from `dir`, the folder
    /// holding the serialized file.
    pub fn data(&self, dir: &Path) -> Result<Vec<u8>> {
        let Some(stream) = &self.stream else {
            return Ok(self.image_data.clone());
        };
        if stream.path.starts_with("archive:") {
            return Err(Error::Unsupported(format!(
                "texture {} streams from an asset bundle ({}); use Texture2D::data_in",
                self.name, stream.path
            )));
        }
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(dir.join(&stream.path))?;
        f.seek(SeekFrom::Start(stream.offset))?;
        let mut data = vec![0; stream.size as usize];
        f.read_exact(&mut data)?;
        Ok(data)
    }

    /// The raw pixel data of every mip level, for a texture read from a bundle: streamed data
    /// is read from the bundle's `.resS` entry.
    pub fn data_in(&self, bundle: &Bundle) -> Result<Vec<u8>> {
        let Some(stream) = &self.stream else {
            return Ok(self.image_data.clone());
        };
        let entry = bundle.entry(&stream.path).ok_or_else(|| {
            Error::Invalid(format!(
                "texture {} streams from {}, which the bundle does not hold",
                self.name, stream.path
            ))
        })?;
        let bytes = bundle.bytes(entry);
        usize::try_from(stream.offset)
            .ok()
            .and_then(|start| bytes.get(start..start.checked_add(stream.size as usize)?))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "texture {} streams past the end of {}",
                    self.name, entry.path
                ))
            })
    }
}

pub(crate) fn read_streaming_info(r: &mut Reader, wide_offset: bool) -> Result<StreamingInfo> {
    let offset = if wide_offset {
        r.u64()?
    } else {
        r.u32()? as u64
    };
    let size = r.u32()?;
    let path = r.aligned_string()?;
    Ok(StreamingInfo { offset, size, path })
}
