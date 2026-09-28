//! Unity's serialized file (`*.assets`, `level*`, `globalgamemanagers`): a header, a table of
//! types, a table of objects, and a list of the other files it points into.

use crate::reader::Reader;
use crate::{Error, Result};

/// Oldest format version read. Version 17 arrived with Unity 5.5.
pub const MIN_VERSION: u32 = 17;
/// Newest format version this crate was written against (Unity 2022.2 and later).
pub const MAX_VERSION: u32 = 22;

/// Unity class IDs this crate cares about, as found in [`ObjectInfo::class_id`].
pub mod class {
    /// `Texture2D`.
    pub const TEXTURE_2D: i32 = 28;
    /// `MonoBehaviour`: a script's data. Its type entries carry an extra script ID.
    pub const MONO_BEHAVIOUR: i32 = 114;
    /// `Sprite`.
    pub const SPRITE: i32 = 213;
    /// `SpriteAtlas`.
    pub const SPRITE_ATLAS: i32 = 687078895;
}

/// One entry of the file's type table.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SerializedType {
    /// The Unity class ID; see [`class`].
    pub class_id: i32,
    /// For script types, an index into the file's script table; otherwise -1.
    pub script_type_index: i16,
}

/// Where one object lives in the file, and what class it is.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ObjectInfo {
    /// The object's ID within this file, used by references ([`crate::PPtr`]).
    pub path_id: i64,
    /// Absolute offset of the object's data in the file.
    pub offset: usize,
    /// Size of the object's data in bytes.
    pub size: usize,
    /// The Unity class ID; see [`class`].
    pub class_id: i32,
}

/// Another file this one references, as listed in its externals table.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct External {
    /// The file's path as Unity wrote it, e.g. `archive:/CAB-.../CAB-...` or
    /// `sharedassets1.assets`.
    pub path: String,
}

/// A parsed serialized file. Holds the whole file's bytes; objects are decoded on demand.
#[non_exhaustive]
pub struct SerializedFile {
    /// Format version, 17 to 22.
    pub version: u32,
    /// The engine release that wrote the file, e.g. `2022.3.62f3`.
    pub unity_version: String,
    /// Unity's `BuildTarget` value.
    pub target_platform: i32,
    /// Whether object data is big-endian.
    pub big_endian: bool,
    /// Whether the file carries type trees. Layouts here are hard-coded either way.
    pub has_type_trees: bool,
    /// The type table.
    pub types: Vec<SerializedType>,
    /// Every object in the file, in file order.
    pub objects: Vec<ObjectInfo>,
    /// Other files this one references.
    pub externals: Vec<External>,
    data: Vec<u8>,
}

impl SerializedFile {
    /// Parse a serialized file from its bytes. An asset bundle is refused with a message
    /// saying so; open those with [`crate::Assets::open`] or [`crate::Bundle::parse`].
    pub fn parse(data: Vec<u8>) -> Result<SerializedFile> {
        if crate::bundle::is_bundle(&data) {
            return Err(Error::Unsupported(
                "this is an asset bundle (UnityFS), not a serialized file; open it with \
                 Assets::open or Bundle::parse"
                    .into(),
            ));
        }
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

    /// Read and parse the file at `path`.
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

    /// The object with this path ID.
    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.objects.iter().find(|o| o.path_id == path_id)
    }

    /// A reader over one object's bytes.
    pub(crate) fn reader(&self, object: &ObjectInfo) -> Reader<'_> {
        Reader::new(
            &self.data[object.offset..object.offset + object.size],
            self.big_endian,
        )
    }

    /// One object's raw bytes.
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
