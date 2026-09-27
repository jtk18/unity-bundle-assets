//! Unity's serialized file (`*.assets`, `level*`, `globalgamemanagers`): a header, a table of
//! types, a table of objects, and a list of the other files it points into.

use crate::reader::Reader;
use crate::{Error, Result};

/// Oldest format version read. Version 17 arrived with Unity 5.5.
pub const MIN_VERSION: u32 = 17;
/// Newest format version this crate was written against (Unity 2022.2 and later).
pub const MAX_VERSION: u32 = 22;

/// Unity class IDs this crate cares about.
pub mod class {
    pub const TEXTURE_2D: i32 = 28;
    pub const MONO_BEHAVIOUR: i32 = 114;
    pub const SPRITE: i32 = 213;
    pub const SPRITE_ATLAS: i32 = 687078895;
}

#[derive(Debug, Clone)]
pub struct SerializedType {
    pub class_id: i32,
    pub script_type_index: i16,
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub path_id: i64,
    /// Absolute offset of the object's data in the file.
    pub offset: usize,
    pub size: usize,
    pub class_id: i32,
}

#[derive(Debug, Clone)]
pub struct External {
    pub path: String,
}

/// A parsed serialized file. Holds the whole file's bytes; objects are decoded on demand.
pub struct SerializedFile {
    pub version: u32,
    pub unity_version: String,
    pub target_platform: i32,
    pub big_endian: bool,
    pub has_type_trees: bool,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    pub externals: Vec<External>,
    data: Vec<u8>,
}

impl SerializedFile {
    pub fn parse(data: Vec<u8>) -> Result<SerializedFile> {
        let mut r = Reader::new(&data, true);
        let _metadata_size = r.u32()?;
        let mut file_size = r.u32()? as u64;
        let version = r.u32()?;
        let mut data_offset = r.u32()? as u64;
        if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
            return Err(Error::Unsupported(format!(
                "serialized file format version {version} (supported: {MIN_VERSION}-{MAX_VERSION})"
            )));
        }
        let big_endian = r.u8()? != 0;
        r.skip(3)?;
        if version >= 22 {
            let _metadata_size = r.u32()?;
            file_size = r.u64()?;
            data_offset = r.u64()?;
            r.skip(8)?;
        }
        if file_size != data.len() as u64 {
            return Err(Error::Invalid(format!(
                "header says {file_size} bytes, file has {}",
                data.len()
            )));
        }

        r.set_big_endian(big_endian);
        let unity_version = r.cstr()?;
        let target_platform = r.i32()?;
        let has_type_trees = r.bool()?;

        let type_count = r.len(4)?;
        let mut types = Vec::with_capacity(type_count);
        for _ in 0..type_count {
            types.push(read_type(&mut r, version, has_type_trees)?);
        }

        let object_count = r.len(20)?;
        let mut objects = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            r.align(4);
            let path_id = r.i64()?;
            let start = if version >= 22 {
                r.u64()?
            } else {
                r.u32()? as u64
            };
            let size = r.u32()? as usize;
            let type_index = r.i32()?;
            let class_id = usize::try_from(type_index)
                .ok()
                .and_then(|i| types.get(i))
                .ok_or_else(|| Error::Invalid(format!("object {path_id} has type {type_index}")))?
                .class_id;
            let offset = (data_offset + start) as usize;
            if offset.checked_add(size).is_none_or(|end| end > data.len()) {
                return Err(Error::Invalid(format!(
                    "object {path_id} runs past the file"
                )));
            }
            objects.push(ObjectInfo {
                path_id,
                offset,
                size,
                class_id,
            });
        }

        // Script references: which MonoScripts the MonoBehaviours use. Not needed here.
        let script_count = r.len(12)?;
        for _ in 0..script_count {
            r.i32()?;
            r.align(4);
            r.i64()?;
        }

        let external_count = r.len(21)?;
        let mut externals = Vec::with_capacity(external_count);
        for _ in 0..external_count {
            r.cstr()?; // always empty
            r.skip(16)?; // GUID
            r.i32()?; // type
            externals.push(External { path: r.cstr()? });
        }

        Ok(SerializedFile {
            version,
            unity_version,
            target_platform,
            big_endian,
            has_type_trees,
            types,
            objects,
            externals,
            data,
        })
    }

    pub fn open(path: &std::path::Path) -> Result<SerializedFile> {
        SerializedFile::parse(std::fs::read(path)?)
    }

    /// The Unity version as numbers: `2022.3.62f3` is `[2022, 3, 62]`.
    pub fn unity_version_numbers(&self) -> [u32; 3] {
        let mut out = [0; 3];
        for (slot, part) in out.iter_mut().zip(
            self.unity_version
                .split(|c: char| !c.is_ascii_digit())
                .filter(|p| !p.is_empty()),
        ) {
            *slot = part.parse().unwrap_or(0);
        }
        out
    }

    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.objects.iter().find(|o| o.path_id == path_id)
    }

    /// A reader over one object's bytes.
    pub fn reader(&self, object: &ObjectInfo) -> Reader<'_> {
        Reader::new(
            &self.data[object.offset..object.offset + object.size],
            self.big_endian,
        )
    }

    pub fn bytes(&self, object: &ObjectInfo) -> &[u8] {
        &self.data[object.offset..object.offset + object.size]
    }

    /// The object's `m_Name`, for classes that start with one (Texture2D, Sprite, ...).
    pub fn name(&self, object: &ObjectInfo) -> Option<String> {
        self.reader(object).aligned_string().ok()
    }
}

fn read_type(r: &mut Reader, version: u32, has_type_trees: bool) -> Result<SerializedType> {
    let class_id = r.i32()?;
    let _stripped = r.bool()?;
    let script_type_index = r.i16()?;
    if class_id == class::MONO_BEHAVIOUR {
        r.skip(16)?; // script ID
    }
    r.skip(16)?; // type hash
    if has_type_trees {
        let nodes = r.len(24)?;
        let strings = r.len(1)?;
        let node_size = if version >= 19 { 32 } else { 24 };
        r.skip(nodes * node_size + strings)?;
        if version >= 21 {
            let deps = r.len(4)?;
            r.skip(deps * 4)?;
        }
    }
    Ok(SerializedType {
        class_id,
        script_type_index,
    })
}
