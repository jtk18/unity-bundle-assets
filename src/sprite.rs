//! `Sprite` and `SpriteAtlas`: which texture a sprite's pixels live in, and where.
//!
//! A sprite packed into a sprite atlas stores a null texture in its own render data; the
//! atlas holds the real texture and rectangle, keyed by the sprite's render-data key.

use crate::reader::Reader;
use crate::serialized::{class, ObjectInfo, SerializedFile};
use crate::{check_release, quoted, Error, LimitKind, Result, Version};

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
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            file_id: r.i32()?,
            path_id: r.i64()?,
        })
    }

    /// Whether this refers to no object.
    #[must_use]
    pub const fn is_null(&self) -> bool {
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
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
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
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
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
#[non_exhaustive]
pub enum Rotation {
    /// Stored as drawn (Unity's `None`).
    Unrotated,
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
    #[must_use]
    pub const fn packed(self) -> bool {
        self.0 & 1 != 0
    }

    /// Tight packing: the sprite's rectangle may hold pixels of its neighbours, and only its
    /// mesh says which pixels are its own.
    #[must_use]
    pub const fn tight(self) -> bool {
        (self.0 >> 1) & 1 == 0
    }

    /// The packing rotation's four bits as stored.
    #[must_use]
    pub const fn rotation_bits(self) -> u32 {
        (self.0 >> 2) & 0xf
    }

    /// The packing rotation, or `None` for a value Unity does not define.
    #[must_use]
    pub const fn rotation(self) -> Option<Rotation> {
        Some(match self.rotation_bits() {
            0 => Rotation::Unrotated,
            1 => Rotation::FlipHorizontal,
            2 => Rotation::FlipVertical,
            3 => Rotation::Rotate180,
            4 => Rotation::Rotate90,
            _ => return None,
        })
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
#[derive(Clone)]
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
    /// Triangles of the sprite's mesh in sprite-local units, three points each; `None` when
    /// the mesh uses a vertex layout this crate does not read or refers to vertices it does
    /// not hold, so a tight-packed sprite cannot be masked.
    pub triangles: Option<Vec<[[f32; 2]; 3]>>,
}

impl std::fmt::Debug for Sprite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sprite")
            .field("path_id", &self.path_id)
            .field("name", &self.name)
            .field("rect", &self.rect)
            .field("pixels_to_units", &self.pixels_to_units)
            .field("pivot", &self.pivot)
            .field("render_data_key", &self.render_data_key)
            .field("atlas", &self.atlas)
            .field("own", &self.own)
            .field("triangles", &self.triangles.as_ref().map(Vec::len))
            .finish()
    }
}

impl Sprite {
    /// Read a `Sprite` object, Unity 2019.1 through 6000.4: final, patch, China and beta
    /// builds (6000.4 finals are read with its betas' layout, which is all that was checked).
    ///
    /// # Errors
    ///
    /// For an engine release outside that range, an object that does not fit the layout, or a
    /// mesh over [`crate::Limits::max_sprite_triangles`].
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Self> {
        Self::read_within(file, object, u64::MAX)
    }

    /// [`Sprite::read`], refusing a mesh of more than `remaining` triangles before building
    /// any of it; `remaining` is what is left of a list's total.
    pub(crate) fn read_within(
        file: &SerializedFile,
        object: &ObjectInfo,
        remaining: u64,
    ) -> Result<Self> {
        check_release(file.unity_version(), "Sprite", OLDEST)?;
        let limit = file.limits().max_sprite_triangles;
        let budget = Budget { limit, remaining };
        let mut r = file.reader_for(object, class::SPRITE)?;
        let v = Version::parse(file.unity_version()).numbers;
        read_sprite(&mut r, file.big_endian(), v, budget, object.path_id())
            .map_err(|e| layout_error(e, "Sprite", object, file))
    }
}

/// How many triangles a mesh may have: its own limit, and what is left of a list's.
#[derive(Clone, Copy)]
struct Budget {
    limit: u64,
    remaining: u64,
}

fn layout_error(e: Error, what: &str, object: &ObjectInfo, file: &SerializedFile) -> Error {
    match e {
        Error::Truncated(_) | Error::BadLength { .. } => Error::Invalid(format!(
            "{what} {} from Unity {} does not fit the layout this crate knows ({e})",
            object.path_id(),
            quoted(file.unity_version())
        )),
        e => e,
    }
}

fn read_sprite(
    r: &mut Reader,
    big_endian: bool,
    v: [u32; 3],
    budget: Budget,
    path_id: i64,
) -> Result<Sprite> {
    let name = r.aligned_string()?;
    let rect = Rect::read(r)?;
    r.skip(8)?; // m_Offset
    r.skip(16)?; // m_Border
    let pixels_to_units = r.f32()?;
    let pivot = [r.f32()?, r.f32()?];
    r.u32()?; // m_Extrude
    r.bool()?; // m_IsPolygon (gone in 6000.5, past the releases this crate reads)
    r.align(4);
    let render_data_key = RenderDataKey::read(r)?;
    for _ in 0..r.len(4)? {
        r.skip_string()?; // m_AtlasTags
    }
    let atlas = PPtr::read(r)?;

    // m_RD
    let texture = PPtr::read(r)?;
    let alpha_texture = PPtr::read(r)?;
    for _ in 0..r.len(16)? {
        PPtr::read(r)?; // secondaryTextures
        r.skip_string()?;
    }
    let triangles = read_mesh(r, big_endian, budget)?;
    let bindposes = r.len(64)?;
    r.skip(bindposes * 64)?;
    let texture_rect = Rect::read(r)?;
    let texture_rect_offset = [r.f32()?, r.f32()?];
    r.skip(8)?; // atlasRectOffset
    let settings = Settings(r.u32()?);
    r.skip(16)?; // uvTransform
    let downscale = r.f32()?;

    // The rest is read only to check that the object ends where these fields do, so a
    // misread layout is an error rather than wrong numbers.
    for _ in 0..r.len(4)? {
        let points = r.len(8)?; // m_PhysicsShape: outlines of Vector2f
        r.skip(points * 8)?;
    }
    let bone_guid_and_colour = v >= [2021, 1, 0];
    for _ in 0..r.len(if bone_guid_and_colour { 48 } else { 40 })? {
        r.skip_string()?; // name
        if bone_guid_and_colour {
            r.skip_string()?; // guid
        }
        r.skip(12 + 16 + 4 + 4)?; // position, rotation, length, parentId
        if bone_guid_and_colour {
            r.skip(4)?; // color
        }
    }
    if v >= [2023, 1, 0] {
        let n = r.len(12)?;
        r.skip(n * 12)?; // m_ScriptableObjects
    }
    r.check_end(|| format!("sprite {}", quoted(&name)))?;

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
        triangles,
    })
}

/// Bytes per component of each `VertexFormat` (Unity 2019 and on).
const fn component_size(format: u8) -> Option<usize> {
    Some(match format {
        0 | 10 | 11 => 4,       // Float, UInt32, SInt32
        1 | 4 | 5 | 8 | 9 => 2, // Float16, UNorm16, SNorm16, UInt16, SInt16
        2 | 3 | 6 | 7 => 1,     // UNorm8, SNorm8, UInt8, SInt8
        _ => return None,
    })
}

/// Reads the sprite mesh (sub-meshes, index buffer, vertex data) and returns its triangles,
/// or `None` when its vertex layout is one this crate does not read or an index points past
/// its vertices. Positions are channel 0,
/// float32; streams are laid out one after another, each padded to 16 bytes, as `UnityPy` does.
fn read_mesh(
    r: &mut Reader,
    big_endian: bool,
    budget: Budget,
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
    let mut total = 0u64;
    for sm in submeshes.iter().filter(|sm| sm.topology == 0) {
        let end = (sm.index_count as usize)
            .checked_mul(2)
            .and_then(|n| n.checked_add(sm.first_byte as usize));
        if end.is_none_or(|end| end > indices.len()) {
            return Err(Error::Invalid(format!(
                "sprite sub-mesh indices {}+{} run past the {}-byte index buffer",
                sm.first_byte,
                sm.index_count,
                indices.len()
            )));
        }
        total = total.saturating_add(u64::from(sm.index_count) / 3);
    }
    // Unity gives each sub-mesh its own indices; letting them overlap would let a small index
    // buffer stand for many triangles.
    let mut spans: Vec<(u64, u64)> = submeshes
        .iter()
        .filter(|sm| sm.topology == 0 && sm.index_count > 0)
        .map(|sm| {
            let first = u64::from(sm.first_byte);
            (first, first + u64::from(sm.index_count) * 2)
        })
        .collect();
    spans.sort_unstable();
    if spans.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(Error::Invalid("sprite sub-meshes share indices".into()));
    }
    Error::limit(LimitKind::SpriteTriangles, total, budget.limit)?;
    Error::limit(LimitKind::TotalTriangles, total, budget.remaining)?;

    let Some(&(pos_stream, pos_offset, 0, 2..=4)) = channels.first() else {
        return Ok(if total == 0 { Some(Vec::new()) } else { None });
    };
    // Stream layout: each stream's stride is the sum of its channels; streams follow one
    // another, each padded to 16 bytes.
    // One pass over the channels for every stream's stride; a channel whose format has no
    // known size spoils its stream.
    let mut strides = [Some(0usize); 256];
    for &(stream, _, format, d) in &channels {
        if d > 0 {
            let slot = &mut strides[usize::from(stream)];
            *slot = slot
                .zip(component_size(format))
                .map(|(s, size)| s + size * d as usize);
        }
    }
    let mut stream_offset = 0usize;
    for s in 0..pos_stream {
        let Some(s_stride) = strides[usize::from(s)] else {
            return Ok(None);
        };
        stream_offset = stream_offset
            .saturating_add(vertex_count.saturating_mul(s_stride))
            .checked_next_multiple_of(16)
            .unwrap_or(usize::MAX);
    }
    let Some(stride) = strides[usize::from(pos_stream)] else {
        return Ok(None);
    };
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
    let mut triangles = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    for sm in submeshes.iter().filter(|sm| sm.topology == 0) {
        let start = sm.first_byte as usize;
        let indices = &indices[start..start + sm.index_count as usize * 2];
        for t in indices.chunks_exact(6) {
            let mut corners = [[0f32; 2]; 3];
            for (corner, c) in corners.iter_mut().zip(t.chunks_exact(2)) {
                let i = if big_endian {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                };
                let i = usize::from(i).saturating_add(sm.base_vertex as usize);
                // A triangle that points past the vertices makes the whole mesh untrustworthy.
                match (i < vertex_count).then(|| vertex(i)).flatten() {
                    Some(p) => *corner = p,
                    None => return Ok(None),
                }
            }
            triangles.push(corners);
        }
    }
    Ok(Some(triangles))
}

/// A `SpriteAtlas` object: where each of its sprites was packed.
#[derive(Clone)]
pub struct SpriteAtlas {
    /// `m_Name`.
    pub name: String,
    entries: Vec<(RenderDataKey, Placement)>,
    index: HashMap<RenderDataKey, usize>,
}

impl std::fmt::Debug for SpriteAtlas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpriteAtlas")
            .field("name", &self.name)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl SpriteAtlas {
    /// Read a `SpriteAtlas` object, Unity 2019.1 through 6000.4: final, patch, China and beta
    /// builds (6000.4 finals are read with its betas' layout, which is all that was checked).
    ///
    /// # Errors
    ///
    /// For an engine release outside that range, or an object that does not fit the layout.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Self> {
        check_release(file.unity_version(), "SpriteAtlas", OLDEST)?;
        let v = file.unity_version_numbers();
        let mut r = file.reader_for(object, class::SPRITE_ATLAS)?;
        read_atlas(&mut r, v).map_err(|e| layout_error(e, "SpriteAtlas", object, file))
    }

    /// Placement by sprite render-data key, in file order.
    #[must_use]
    pub fn entries(&self) -> &[(RenderDataKey, Placement)] {
        &self.entries
    }

    /// The placement filed under a sprite's render-data key (the first, if repeated).
    #[must_use]
    pub fn placement(&self, key: &RenderDataKey) -> Option<Placement> {
        self.index.get(key).map(|&i| self.entries[i].1)
    }
}

fn read_atlas(r: &mut Reader, v: [u32; 3]) -> Result<SpriteAtlas> {
    let name = r.aligned_string()?;
    let packed = r.len(12)?;
    r.skip(packed * 12)?; // m_PackedSprites
    for _ in 0..r.len(4)? {
        r.skip_string()?; // m_PackedSpriteNamesToIndex
    }
    // An entry is at least 104 bytes: key 24, two pointers 24, rect 16, offsets 16, UV
    // transform 16, downscale and settings 8; from 2020.2 also a list count.
    let secondary = v >= [2020, 2, 0];
    let count = r.len(if secondary { 108 } else { 104 })?;
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
        if secondary {
            for _ in 0..r.len(16)? {
                PPtr::read(r)?;
                r.skip_string()?;
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
    r.skip_string()?; // m_Tag
    r.bool()?; // m_IsVariant
    r.align(4);
    // The last field ends the object. Bytes left over mean the layout was misread.
    r.check_end(|| format!("sprite atlas {}", quoted(&name)))?;
    Ok(SpriteAtlas {
        name,
        entries,
        index,
    })
}
