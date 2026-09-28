//! `Sprite` and `SpriteAtlas`: which texture a sprite's pixels live in, and where.
//!
//! A sprite packed into a sprite atlas stores a null texture in its own render data; the
//! atlas holds the real texture and rectangle, keyed by the sprite's render-data key.

use crate::reader::Reader;
use crate::serialized::{ObjectInfo, SerializedFile};
use crate::{check_release, Error, Limits, Result};

use std::collections::HashMap;

/// Sprites and sprite atlases are read from this release on.
const OLDEST: [u32; 3] = [2019, 1, 0];

/// A reference to an object, possibly in another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PPtr {
    /// 0 for this file, otherwise 1 + an index into [`SerializedFile::externals`].
    pub file_id: i32,
    /// The object's [`crate::ObjectInfo::path_id`]; 0 means no object.
    pub path_id: i64,
}

impl PPtr {
    fn read(r: &mut Reader) -> Result<PPtr> {
        Ok(PPtr {
            file_id: r.i32()?,
            path_id: r.i64()?,
        })
    }

    /// Whether this refers to no object.
    pub fn is_null(&self) -> bool {
        self.path_id == 0
    }
}

/// The key a sprite atlas files a sprite's render data under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RenderDataKey {
    /// The sprite's GUID.
    pub guid: [u8; 16],
    /// Its local ID.
    pub id: i64,
}

impl RenderDataKey {
    fn read(r: &mut Reader) -> Result<RenderDataKey> {
        Ok(RenderDataKey {
            guid: r.take(16)?.try_into().unwrap(),
            id: r.i64()?,
        })
    }
}

/// A rectangle in pixels, origin at the bottom left as Unity stores it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Bottom edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    fn read(r: &mut Reader) -> Result<Rect> {
        Ok(Rect {
            x: r.f32()?,
            y: r.f32()?,
            width: r.f32()?,
            height: r.f32()?,
        })
    }
}

/// How a sprite sits in its texture (`SpriteSettings`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Settings(pub u32);

/// How a packed sprite was turned to fit its texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    /// Stored as drawn.
    None,
    /// Mirrored left to right.
    FlipHorizontal,
    /// Mirrored top to bottom.
    FlipVertical,
    /// Turned half a turn.
    Rotate180,
    /// Turned a quarter turn.
    Rotate90,
}

impl Settings {
    /// Whether the sprite was packed, so rotation applies.
    pub fn packed(self) -> bool {
        self.0 & 1 != 0
    }

    /// Tight packing: the sprite's rectangle may hold pixels of its neighbours, and only its
    /// mesh says which pixels are its own.
    pub fn tight(self) -> bool {
        (self.0 >> 1) & 1 == 0
    }

    /// The packing rotation. Values Unity does not define read as [`Rotation::None`].
    pub fn rotation(self) -> Rotation {
        match (self.0 >> 2) & 0xf {
            1 => Rotation::FlipHorizontal,
            2 => Rotation::FlipVertical,
            3 => Rotation::Rotate180,
            4 => Rotation::Rotate90,
            _ => Rotation::None,
        }
    }
}

/// Where a sprite's pixels are: the texture, the rectangle within it, and how they're packed.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Placement {
    /// The texture holding the pixels.
    pub texture: PPtr,
    /// A separate texture holding the alpha channel, or null.
    pub alpha_texture: PPtr,
    /// The sprite's pixels within that texture.
    pub texture_rect: Rect,
    /// Offset of `texture_rect` within the sprite's full rect.
    pub texture_rect_offset: [f32; 2],
    /// Packing flags.
    pub settings: Settings,
    /// Scale of the atlas texture relative to the sprite's rect; 1 when not downscaled.
    pub downscale: f32,
}

/// A `Sprite` object.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Sprite {
    /// The object's path ID.
    pub path_id: i64,
    /// `m_Name`. Not unique: games reuse names.
    pub name: String,
    /// The sprite's full rectangle, before any transparent border was trimmed.
    pub rect: Rect,
    /// Pixels per world unit.
    pub pixels_to_units: f32,
    /// Pivot as a fraction of `rect`.
    pub pivot: [f32; 2],
    /// The key its atlas files it under.
    pub render_data_key: RenderDataKey,
    /// The atlas holding it, or null.
    pub atlas: PPtr,
    /// The sprite's own render data. For an atlased sprite, the texture here is null and
    /// the atlas's entry is the one to use.
    pub own: Placement,
    /// Triangles of the sprite's mesh in sprite-local units, three points each.
    pub triangles: Vec<[[f32; 2]; 3]>,
    /// The mesh is in a vertex layout this crate does not read, so `triangles` is empty and a
    /// tight-packed sprite cannot be masked.
    pub mesh_unreadable: bool,
}

impl Sprite {
    /// Read a `Sprite` object, Unity 2019.1 through 6000.5.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Sprite> {
        let v = file.unity_version_numbers();
        check_release(file.unity_version(), v, "Sprite", OLDEST)?;
        let mut r = file.reader(object)?;
        read_sprite(
            &mut r,
            v,
            file.big_endian(),
            &file.limits(),
            object.path_id(),
        )
        .map_err(|e| layout_error(e, "Sprite", object, file))
    }
}

fn layout_error(e: Error, what: &str, object: &ObjectInfo, file: &SerializedFile) -> Error {
    match e {
        Error::Truncated(_) | Error::BadLength { .. } => Error::Invalid(format!(
            "{what} {} from Unity {} does not fit the layout this crate knows ({e})",
            object.path_id(),
            file.unity_version()
        )),
        e => e,
    }
}

fn read_sprite(
    r: &mut Reader,
    v: [u32; 3],
    big_endian: bool,
    limits: &Limits,
    path_id: i64,
) -> Result<Sprite> {
    let name = r.aligned_string()?;
    let rect = Rect::read(r)?;
    r.skip(8)?; // m_Offset
    r.skip(16)?; // m_Border
    let pixels_to_units = r.f32()?;
    let pivot = [r.f32()?, r.f32()?];
    r.u32()?; // m_Extrude
    if v < [6000, 5, 0] {
        r.bool()?; // m_IsPolygon, gone in 6000.5
    }
    r.align(4);
    let render_data_key = RenderDataKey::read(r)?;
    for _ in 0..r.len(4)? {
        r.aligned_string()?; // m_AtlasTags
    }
    let atlas = PPtr::read(r)?;

    // m_RD
    let texture = PPtr::read(r)?;
    let alpha_texture = PPtr::read(r)?;
    for _ in 0..r.len(16)? {
        PPtr::read(r)?; // secondaryTextures
        r.aligned_string()?;
    }
    let triangles = read_mesh(r, big_endian, limits)?;
    let bindposes = r.len(64)?;
    r.skip(bindposes * 64)?;
    let texture_rect = Rect::read(r)?;
    let texture_rect_offset = [r.f32()?, r.f32()?];
    r.skip(8)?; // atlasRectOffset
    let settings = Settings(r.u32()?);
    r.skip(16)?; // uvTransform
    let downscale = r.f32()?;

    Ok(Sprite {
        path_id,
        name,
        rect,
        pixels_to_units,
        pivot,
        render_data_key,
        atlas,
        own: Placement {
            texture,
            alpha_texture,
            texture_rect,
            texture_rect_offset,
            settings,
            downscale,
        },
        mesh_unreadable: triangles.is_none(),
        triangles: triangles.unwrap_or_default(),
    })
}

/// Bytes per component of each `VertexFormat` (Unity 2019 and on).
fn component_size(format: u8) -> Option<usize> {
    Some(match format {
        0 | 10 | 11 => 4,       // Float, UInt32, SInt32
        1 | 4 | 5 | 8 | 9 => 2, // Float16, UNorm16, SNorm16, UInt16, SInt16
        2 | 3 | 6 | 7 => 1,     // UNorm8, SNorm8, UInt8, SInt8
        _ => return None,
    })
}

/// Reads the sprite mesh (sub-meshes, index buffer, vertex data) and returns its triangles,
/// or `None` when its vertex layout is one this crate does not read. Positions are channel 0,
/// float32; streams are laid out one after another, each padded to 16 bytes, as UnityPy does.
fn read_mesh(
    r: &mut Reader,
    big_endian: bool,
    limits: &Limits,
) -> Result<Option<Vec<[[f32; 2]; 3]>>> {
    struct SubMesh {
        first_byte: u32,
        index_count: u32,
        topology: i32,
        base_vertex: u32,
    }
    let mut submeshes = Vec::new();
    for _ in 0..r.len(48)? {
        let first_byte = r.u32()?;
        let index_count = r.u32()?;
        let topology = r.i32()?;
        let base_vertex = r.u32()?;
        r.u32()?; // firstVertex
        r.u32()?; // vertexCount
        r.skip(24)?; // localAABB
        submeshes.push(SubMesh {
            first_byte,
            index_count,
            topology,
            base_vertex,
        });
    }
    let indices = r.byte_array()?;
    let vertex_count = r.u32()? as usize;
    let channel_count = r.len(4)?;
    let mut channels = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let [stream, offset, format, dimension] = r.take(4)?.try_into().unwrap();
        channels.push((stream, offset, format, dimension & 0xf));
    }
    let vertex_data = r.byte_array()?;

    // Every index range must lie inside the index buffer, and the triangles they make are
    // capped before any is built.
    let mut total = 0usize;
    for sm in submeshes.iter().filter(|sm| sm.topology == 0) {
        let end = (sm.first_byte as usize).checked_add(sm.index_count as usize * 2);
        if end.is_none_or(|end| end > indices.len()) {
            return Err(Error::Invalid(format!(
                "sprite sub-mesh indices {}+{} run past the {}-byte index buffer",
                sm.first_byte,
                sm.index_count,
                indices.len()
            )));
        }
        total = total.saturating_add(sm.index_count as usize / 3);
    }
    if total > limits.max_sprite_triangles {
        return Err(Error::LimitExceeded {
            what: "sprite mesh triangles",
            value: total as u64,
            limit: limits.max_sprite_triangles as u64,
        });
    }

    let Some(&(pos_stream, pos_offset, 0, dims @ 2..=4)) = channels.first() else {
        return Ok(if total == 0 { Some(Vec::new()) } else { None });
    };
    // Stream layout: each stream's stride is the sum of its channels; streams follow one
    // another, each padded to 16 bytes.
    let mut stream_offset = 0usize;
    let mut stride = 0usize;
    for s in 0..=pos_stream {
        let mut s_stride = 0usize;
        for &(stream, _, format, d) in &channels {
            if stream == s && d > 0 {
                let Some(size) = component_size(format) else {
                    return Ok(None);
                };
                s_stride += size * d as usize;
            }
        }
        if s == pos_stream {
            stride = s_stride;
        } else {
            stream_offset = stream_offset
                .saturating_add(vertex_count.saturating_mul(s_stride))
                .checked_next_multiple_of(16)
                .unwrap_or(usize::MAX);
        }
    }
    if stride < dims as usize * 4 {
        return Ok(None);
    }
    let f32_at = |b: &[u8]| {
        let b: [u8; 4] = b.try_into().unwrap();
        if big_endian {
            f32::from_be_bytes(b)
        } else {
            f32::from_le_bytes(b)
        }
    };
    let vertex = |i: usize| -> Option<[f32; 2]> {
        let at = i
            .checked_mul(stride)?
            .checked_add(stream_offset)?
            .checked_add(pos_offset as usize)?;
        let b = vertex_data.get(at..at.checked_add(8)?)?;
        Some([f32_at(&b[0..4]), f32_at(&b[4..8])])
    };
    let mut triangles = Vec::with_capacity(total);
    for sm in submeshes.iter().filter(|sm| sm.topology == 0) {
        let start = sm.first_byte as usize;
        let idx: Vec<usize> = indices[start..start + sm.index_count as usize * 2]
            .chunks_exact(2)
            .map(|c| {
                let i = if big_endian {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                };
                (i as usize).saturating_add(sm.base_vertex as usize)
            })
            .collect();
        for t in idx.chunks_exact(3) {
            if t.iter().any(|&i| i >= vertex_count) {
                continue;
            }
            if let (Some(a), Some(b), Some(c)) = (vertex(t[0]), vertex(t[1]), vertex(t[2])) {
                triangles.push([a, b, c]);
            }
        }
    }
    Ok(Some(triangles))
}

/// A `SpriteAtlas` object: where each of its sprites was packed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SpriteAtlas {
    /// `m_Name`.
    pub name: String,
    /// Placement by sprite render-data key, in file order.
    pub entries: Vec<(RenderDataKey, Placement)>,
    index: HashMap<RenderDataKey, usize>,
}

impl SpriteAtlas {
    /// Read a `SpriteAtlas` object, Unity 2019.1 through 6000.5.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<SpriteAtlas> {
        let v = file.unity_version_numbers();
        check_release(file.unity_version(), v, "SpriteAtlas", OLDEST)?;
        let mut r = file.reader(object)?;
        read_atlas(&mut r, v).map_err(|e| layout_error(e, "SpriteAtlas", object, file))
    }

    /// The placement filed under a sprite's render-data key (the first, if repeated).
    pub fn placement(&self, key: &RenderDataKey) -> Option<Placement> {
        self.index.get(key).map(|&i| self.entries[i].1)
    }
}

fn read_atlas(r: &mut Reader, v: [u32; 3]) -> Result<SpriteAtlas> {
    let name = r.aligned_string()?;
    let packed = r.len(12)?;
    r.skip(packed * 12)?; // m_PackedSprites
    for _ in 0..r.len(4)? {
        r.aligned_string()?; // m_PackedSpriteNamesToIndex
    }
    let count = r.len(24)?;
    let mut entries = Vec::with_capacity(count);
    let mut index = HashMap::with_capacity(count);
    for i in 0..count {
        let key = RenderDataKey::read(r)?;
        let texture = PPtr::read(r)?;
        let alpha_texture = PPtr::read(r)?;
        let texture_rect = Rect::read(r)?;
        let texture_rect_offset = [r.f32()?, r.f32()?];
        r.skip(8)?; // atlasRectOffset
        r.skip(16)?; // uvTransform
        let downscale = r.f32()?;
        let settings = Settings(r.u32()?);
        if v >= [2020, 2, 0] {
            for _ in 0..r.len(16)? {
                PPtr::read(r)?;
                r.aligned_string()?;
            }
        }
        index.entry(key).or_insert(i);
        entries.push((
            key,
            Placement {
                texture,
                alpha_texture,
                texture_rect,
                texture_rect_offset,
                settings,
                downscale,
            },
        ));
    }
    Ok(SpriteAtlas {
        name,
        entries,
        index,
    })
}
