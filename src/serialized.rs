//! Unity's serialized file (`*.assets`, `level*`, `globalgamemanagers`): a header, a table of
//! types, a table of objects, and a list of the other files it points into.

use crate::bundle::{Bundle, Entry};
use crate::reader::Reader;
use crate::{check_version_string, Error, LimitKind, Limits, Result, Version};

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// Oldest format version read. Version 17 arrived with Unity 5.5.
const MIN_VERSION: u32 = 17;
/// Newest format version this crate was written against.
const MAX_VERSION: u32 = 22;

/// Unity class IDs this crate cares about, as returned by [`ObjectInfo::class_id`].
pub mod class {
    /// `Texture2D`.
    pub const TEXTURE_2D: i32 = 28;
    /// `MonoBehaviour`: a script's data. Its type entries carry an extra script ID.
    pub const MONO_BEHAVIOUR: i32 = 114;
    /// `Sprite`.
    pub const SPRITE: i32 = 213;
    /// `SpriteAtlas`.
    pub const SPRITE_ATLAS: i32 = 687_078_895;
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
pub struct ObjectInfo {
    path_id: i64,
    offset: usize,
    size: usize,
    class_id: i32,
}

impl ObjectInfo {
    /// The object's ID within this file, used by references ([`crate::PPtr`]).
    #[must_use]
    pub const fn path_id(&self) -> i64 {
        self.path_id
    }

    /// Size of the object's data in bytes.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// The Unity class ID; see [`class`].
    #[must_use]
    pub const fn class_id(&self) -> i32 {
        self.class_id
    }
}

/// Another file this one references, as listed in its externals table.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct External {
    /// The file's path as Unity wrote it, e.g. `archive:/CAB-.../CAB-...` or
    /// `sharedassets1.assets`.
    pub path: String,
}

/// The file's bytes: its own, or a range of the bundle holding it.
enum Bytes {
    Owned(Vec<u8>),
    InBundle(Arc<Bundle>, Range<usize>),
}

impl Bytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(v) => v,
            Self::InBundle(b, r) => &b.data_ref()[r.clone()],
        }
    }
}

/// A parsed serialized file. Holds the whole file's bytes (or shares its bundle's); objects are
/// decoded on demand.
pub struct SerializedFile {
    version: u32,
    unity_version: String,
    unity_numbers: [u32; 3],
    target_platform: i32,
    big_endian: bool,
    has_type_trees: bool,
    types: Vec<SerializedType>,
    objects: Vec<ObjectInfo>,
    index: HashMap<i64, usize>,
    externals: Vec<External>,
    data: Bytes,
    limits: Limits,
}

impl std::fmt::Debug for SerializedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerializedFile")
            .field("version", &self.version)
            .field("unity_version", &self.unity_version)
            .field("target_platform", &self.target_platform)
            .field("big_endian", &self.big_endian)
            .field("objects", &self.objects.len())
            .finish_non_exhaustive()
    }
}

/// What the header and metadata say, before the bytes are wrapped.
struct Parsed {
    version: u32,
    unity_version: String,
    target_platform: i32,
    big_endian: bool,
    has_type_trees: bool,
    types: Vec<SerializedType>,
    objects: Vec<ObjectInfo>,
    externals: Vec<External>,
}

impl SerializedFile {
    /// Parse a serialized file from its bytes, with the default [`Limits`]. An asset bundle
    /// is refused with a message saying so; open those with [`crate::Assets::open`] or
    /// [`crate::Bundle::parse`].
    ///
    /// # Errors
    ///
    /// When the data is not a serialized file this crate reads, or contradicts itself.
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        Self::parse_with(data, Limits::default())
    }

    /// [`SerializedFile::parse`] under `limits`.
    ///
    /// # Errors
    ///
    /// As [`SerializedFile::parse`].
    pub fn parse_with(data: Vec<u8>, limits: Limits) -> Result<Self> {
        let parsed = parse(&data, &limits)?;
        Ok(Self::build(parsed, Bytes::Owned(data), limits))
    }

    /// Read and parse the file at `path`, with the default [`Limits`].
    ///
    /// # Errors
    ///
    /// When the file cannot be read or is over the size limit, and as
    /// [`SerializedFile::parse`].
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_with(path, Limits::default())
    }

    /// [`SerializedFile::open`] under `limits`.
    ///
    /// # Errors
    ///
    /// As [`SerializedFile::open`].
    pub fn open_with(path: impl AsRef<std::path::Path>, limits: Limits) -> Result<Self> {
        let data = crate::file::read_limited(path.as_ref(), limits.max_file_size)?;
        Self::parse_with(data, limits)
    }

    /// Parse the serialized file in a bundle entry, sharing the bundle's bytes. A file whose
    /// engine version was stripped takes the bundle's.
    pub(crate) fn in_bundle(bundle: Arc<Bundle>, entry: &Entry) -> Result<Self> {
        let bytes = bundle
            .bytes(entry)
            .ok_or_else(|| Error::NotFound(format!("entry {:?} in this bundle", entry.path())))?;
        let mut parsed = parse(bytes, &bundle.limits())?;
        if Version::parse(&parsed.unity_version).stripped() {
            parsed.unity_version = bundle.unity_revision().to_string();
        }
        let limits = bundle.limits();
        let range = Bundle::range_of(entry);
        Ok(Self::build(parsed, Bytes::InBundle(bundle, range), limits))
    }

    fn build(p: Parsed, data: Bytes, limits: Limits) -> Self {
        let index = p
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| (o.path_id, i))
            .collect();
        Self {
            version: p.version,
            unity_numbers: Version::parse(&p.unity_version).numbers,
            unity_version: p.unity_version,
            target_platform: p.target_platform,
            big_endian: p.big_endian,
            has_type_trees: p.has_type_trees,
            types: p.types,
            objects: p.objects,
            index,
            externals: p.externals,
            data,
            limits,
        }
    }

    /// Format version, 17 to 22.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// The engine release that wrote the file, e.g. `2022.3.62f3`. In a bundle, a stripped
    /// version is replaced by the bundle's.
    #[must_use]
    pub fn unity_version(&self) -> &str {
        &self.unity_version
    }

    /// The Unity version as numbers: `2022.3.62f3` is `[2022, 3, 62]`.
    #[must_use]
    pub const fn unity_version_numbers(&self) -> [u32; 3] {
        self.unity_numbers
    }

    /// Unity's `BuildTarget` value.
    #[must_use]
    pub const fn target_platform(&self) -> i32 {
        self.target_platform
    }

    /// Whether object data is big-endian.
    #[must_use]
    pub const fn big_endian(&self) -> bool {
        self.big_endian
    }

    /// Whether the file carries type trees. Layouts here are hard-coded either way.
    #[must_use]
    pub const fn has_type_trees(&self) -> bool {
        self.has_type_trees
    }

    /// The type table.
    #[must_use]
    pub fn types(&self) -> &[SerializedType] {
        &self.types
    }

    /// Every object in the file, in file order.
    #[must_use]
    pub fn objects(&self) -> &[ObjectInfo] {
        &self.objects
    }

    /// Other files this one references.
    #[must_use]
    pub fn externals(&self) -> &[External] {
        &self.externals
    }

    /// The limits the file was parsed under.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// The object with this path ID.
    #[must_use]
    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.index.get(&path_id).map(|&i| &self.objects[i])
    }

    /// One object's raw bytes, or `None` for an object from another file that does not fit
    /// this one.
    #[must_use]
    pub fn bytes(&self, object: &ObjectInfo) -> Option<&[u8]> {
        self.data
            .as_slice()
            .get(object.offset..object.offset.checked_add(object.size)?)
    }

    /// A reader over an object of `class`'s bytes, or [`Error::WrongClass`].
    pub(crate) fn reader_for(&self, object: &ObjectInfo, class: i32) -> Result<Reader<'_>> {
        if object.class_id != class {
            return Err(Error::WrongClass {
                path_id: object.path_id,
                found: object.class_id,
                expected: class,
            });
        }
        self.reader(object)
    }

    /// A reader over one object's bytes.
    pub(crate) fn reader(&self, object: &ObjectInfo) -> Result<Reader<'_>> {
        let bytes = self
            .bytes(object)
            .ok_or_else(|| Error::NotFound(format!("object {} in this file", object.path_id)))?;
        Ok(Reader::new(bytes, self.big_endian))
    }

    /// The object's `m_Name`, for classes that start with one (`Texture2D`, Sprite, ...).
    #[must_use]
    pub fn name(&self, object: &ObjectInfo) -> Option<String> {
        self.reader(object).ok()?.aligned_string().ok()
    }
}

fn parse(data: &[u8], limits: &Limits) -> Result<Parsed> {
    if crate::bundle::is_bundle(data) {
        return Err(Error::Unsupported(
            "this is an asset bundle (UnityFS), not a serialized file; open it with \
             Assets::open or Bundle::parse"
                .into(),
        ));
    }
    for other in ["UnityWeb", "UnityRaw", "UnityArchive"] {
        if data.starts_with(other.as_bytes()) {
            return Err(Error::Unsupported(format!("{other} bundle container")));
        }
    }
    let mut r = Reader::new(data, true);
    let _metadata_size = r.u32()?;
    let mut file_size = u64::from(r.u32()?);
    let version = r.u32()?;
    let mut data_offset = u64::from(r.u32()?);
    if !(MIN_VERSION..=MAX_VERSION).contains(&version) {
        // Older and newer Unity files have a plausible header too (a v22-style header leaves
        // the old size field 0); anything else is not Unity at all.
        let plausible = ((9..MIN_VERSION).contains(&version) && file_size == data.len() as u64)
            || ((MAX_VERSION + 1..=40).contains(&version) && file_size == 0);
        return Err(if plausible {
            Error::Unsupported(format!(
                "serialized file format version {version} (supported: {MIN_VERSION}-{MAX_VERSION})"
            ))
        } else {
            Error::NotUnity(format!(
                "no serialized-file header (format field {version})"
            ))
        });
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
    if data_offset > file_size {
        return Err(Error::Invalid(format!(
            "object data starts at {data_offset}, past the end of the file"
        )));
    }

    r.set_big_endian(big_endian);
    let unity_version = r.cstr()?;
    check_version_string(&unity_version, "serialized file")?;
    let target_platform = r.i32()?;
    let has_type_trees = r.bool()?;

    // Smallest records: a type is 23 bytes, an object 20 (24 from v22), a script 12, an
    // external 22.
    let type_count = r.len(23)?;
    let mut types = Vec::with_capacity(type_count);
    for _ in 0..type_count {
        types.push(read_type(&mut r, version, has_type_trees)?);
    }

    let object_count = r.len(if version >= 22 { 24 } else { 20 })?;
    Error::limit(LimitKind::Objects, object_count as u64, limits.max_objects)?;
    let mut objects = Vec::with_capacity(object_count);
    for _ in 0..object_count {
        r.align(4);
        let path_id = r.i64()?;
        let start = if version >= 22 {
            r.u64()?
        } else {
            u64::from(r.u32()?)
        };
        let size = r.u32()? as usize;
        let type_index = r.i32()?;
        let class_id = usize::try_from(type_index)
            .ok()
            .and_then(|i| types.get(i))
            .ok_or_else(|| Error::Invalid(format!("object {path_id} has type {type_index}")))?
            .class_id;
        let offset = data_offset
            .checked_add(start)
            .and_then(|o| usize::try_from(o).ok())
            .filter(|&o| o.checked_add(size).is_some_and(|end| end <= data.len()))
            .ok_or_else(|| Error::Invalid(format!("object {path_id} runs past the file")))?;
        objects.push(ObjectInfo {
            path_id,
            offset,
            size,
            class_id,
        });
    }

    // Unity never writes two objects over the same bytes, or two with one ID. Allowing either
    // lets a small file name one large object many times over.
    let mut ids: Vec<i64> = objects.iter().map(|o| o.path_id).collect();
    ids.sort_unstable();
    if let Some([id, _]) = ids.windows(2).find(|w| w[0] == w[1]) {
        return Err(Error::Invalid(format!("two objects have path ID {id}")));
    }
    drop(ids);
    let mut spans: Vec<(usize, usize, i64)> = objects
        .iter()
        .filter(|o| o.size > 0)
        .map(|o| (o.offset, o.offset + o.size, o.path_id))
        .collect();
    spans.sort_unstable();
    if let Some(w) = spans.windows(2).find(|w| w[1].0 < w[0].1) {
        return Err(Error::Invalid(format!(
            "objects {} and {} overlap",
            w[0].2, w[1].2
        )));
    }
    drop(spans);

    // Script references: which MonoScripts the MonoBehaviours use. Not needed here.
    let script_count = r.len(12)?;
    for _ in 0..script_count {
        r.i32()?;
        r.align(4);
        r.i64()?;
    }

    let external_count = r.len(22)?;
    let mut externals = Vec::with_capacity(external_count);
    for _ in 0..external_count {
        r.cstr()?; // always empty
        r.skip(16)?; // GUID
        r.i32()?; // type
        externals.push(External { path: r.cstr()? });
    }

    Ok(Parsed {
        version,
        unity_version,
        target_platform,
        big_endian,
        has_type_trees,
        types,
        objects,
        externals,
    })
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
        let at = r.pos();
        let nodes = r.len(24)?;
        let strings = r.len(1)?;
        let node_size = if version >= 19 { 32 } else { 24 };
        let skip = nodes
            .checked_mul(node_size)
            .and_then(|n| n.checked_add(strings))
            .ok_or(Error::BadLength {
                at,
                len: nodes as i64,
            })?;
        r.skip(skip)?;
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
