//! `Sprite` and `SpriteAtlas`: which texture a sprite's pixels live in, and where.
//!
//! A sprite packed into a sprite atlas stores a null texture in its own render data; the
//! atlas holds the real texture and rectangle, keyed by the sprite's render-data key.

use crate::reader::Reader;
use crate::serialized::{ObjectInfo, SerializedFile};
use crate::{Error, Result};

/// A reference to an object, possibly in another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settings(pub u32);

/// How a packed sprite was turned to fit its texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The packing rotation.
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
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Placement {
    /// The texture holding the pixels.
    pub texture: PPtr,
    /// The sprite's pixels within that texture.
    pub texture_rect: Rect,
    /// Offset of `texture_rect` within the sprite's full rect.
    pub texture_rect_offset: [f32; 2],
    /// Packing flags.
    pub settings: Settings,
}

/// A `Sprite` object.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Sprite {
    /// `m_Name`.
    pub name: String,
    /// The sprite's full rectangle, before any transparent border was trimmed.
    pub rect: Rect,
    /// Pixels per world unit.
    pub pixels_to_units: f32,
    /// Pivot as a fraction of `rect`.
    pub pivot: [f32; 2],
    /// The key its atlas files it under.
    pub render_data_key: ([u8; 16], i64),
    /// The atlas holding it, or null.
    pub atlas: PPtr,
    /// The sprite's own render data. For an atlased sprite, the texture here is null and
    /// the atlas's entry is the one to use.
    pub own: Placement,
    /// Triangles of the sprite's mesh in sprite-local units, three points each.
    pub triangles: Vec<[[f32; 2]; 3]>,
}

fn at_least(file: &SerializedFile, ma: u32, mi: u32) -> bool {
    let [major, minor, _] = file.unity_version_numbers();
    (major, minor) >= (ma, mi)
}

impl Sprite {
    /// Read a `Sprite` object. Unity 2019.1 and later.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<Sprite> {
        if !at_least(file, 2019, 1) {
            return Err(Error::Unsupported(format!(
                "Sprite from Unity {} (needs 2019.1 or later)",
                file.unity_version
            )));
        }
        let mut r = file.reader(object);
        let name = r.aligned_string()?;
        let rect = Rect::read(&mut r)?;
        r.skip(8)?; // m_Offset
        r.skip(16)?; // m_Border
        let pixels_to_units = r.f32()?;
        let pivot = [r.f32()?, r.f32()?];
        r.u32()?; // m_Extrude
        r.bool()?; // m_IsPolygon
        r.align(4);
        let guid: [u8; 16] = r.take(16)?.try_into().unwrap();
        let render_data_key = (guid, r.i64()?);
        for _ in 0..r.len(4)? {
            r.aligned_string()?; // m_AtlasTags
        }
        let atlas = PPtr::read(&mut r)?;

        // m_RD
        let texture = PPtr::read(&mut r)?;
        PPtr::read(&mut r)?; // alphaTexture
        for _ in 0..r.len(16)? {
            PPtr::read(&mut r)?; // secondaryTextures
            r.aligned_string()?;
        }
        let triangles = read_mesh(&mut r)?;
        let bindposes = r.len(64)?;
        r.skip(bindposes * 64)?;
        let texture_rect = Rect::read(&mut r)?;
        let texture_rect_offset = [r.f32()?, r.f32()?];
        r.skip(8)?; // atlasRectOffset
        let settings = Settings(r.u32()?);

        Ok(Sprite {
            name,
            rect,
            pixels_to_units,
            pivot,
            render_data_key,
            atlas,
            own: Placement {
                texture,
                texture_rect,
                texture_rect_offset,
                settings,
            },
            triangles,
        })
    }
}

/// Reads the sprite mesh (sub-meshes, index buffer, vertex data) and returns its triangles.
/// Positions are the vertex stream's first channel, three floats each.
fn read_mesh(r: &mut Reader) -> Result<Vec<[[f32; 2]; 3]>> {
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
    let channels = r.len(4)?;
    let mut position = None;
    for i in 0..channels {
        let [stream, offset, format, dimension] = r.take(4)?.try_into().unwrap();
        if i == 0 {
            position = Some((stream, offset, format, dimension & 0xf));
        }
    }
    let vertex_data = r.byte_array()?;

    // Channel 0 is position: stream 0, offset 0, float32 (format 0), 3 dimensions. Every
    // sprite mesh Unity writes looks like this; anything else is left unread.
    let Some((0, 0, 0, dims @ 2..=4)) = position else {
        return Ok(Vec::new());
    };
    let stride = dims as usize * 4;
    let vertex = |i: usize| -> Option<[f32; 2]> {
        let at = i.checked_mul(stride)?;
        let b = vertex_data.get(at..at + 8)?;
        Some([
            f32::from_le_bytes(b[0..4].try_into().unwrap()),
            f32::from_le_bytes(b[4..8].try_into().unwrap()),
        ])
    };
    let mut triangles = Vec::new();
    for sm in submeshes.iter().filter(|sm| sm.topology == 0) {
        let start = sm.first_byte as usize;
        let idx: Vec<usize> = indices
            .get(start..start + sm.index_count as usize * 2)
            .unwrap_or_default()
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]) as usize + sm.base_vertex as usize)
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
    Ok(triangles)
}

/// A `SpriteAtlas` object: where each of its sprites was packed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SpriteAtlas {
    /// `m_Name`.
    pub name: String,
    /// Placement by sprite render-data key.
    pub entries: Vec<(([u8; 16], i64), Placement)>,
}

impl SpriteAtlas {
    /// Read a `SpriteAtlas` object.
    pub fn read(file: &SerializedFile, object: &ObjectInfo) -> Result<SpriteAtlas> {
        let mut r = file.reader(object);
        let name = r.aligned_string()?;
        let packed = r.len(12)?;
        r.skip(packed * 12)?; // m_PackedSprites
        for _ in 0..r.len(4)? {
            r.aligned_string()?; // m_PackedSpriteNamesToIndex
        }
        let count = r.len(24)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let guid: [u8; 16] = r.take(16)?.try_into().unwrap();
            let key = (guid, r.i64()?);
            let texture = PPtr::read(&mut r)?;
            PPtr::read(&mut r)?; // alphaTexture
            let texture_rect = Rect::read(&mut r)?;
            let texture_rect_offset = [r.f32()?, r.f32()?];
            r.skip(8)?; // atlasRectOffset
            r.skip(16)?; // uvTransform
            r.f32()?; // downscaleMultiplier
            let settings = Settings(r.u32()?);
            if at_least(file, 2020, 2) {
                for _ in 0..r.len(16)? {
                    PPtr::read(&mut r)?;
                    r.aligned_string()?;
                }
            }
            entries.push((
                key,
                Placement {
                    texture,
                    texture_rect,
                    texture_rect_offset,
                    settings,
                },
            ));
        }
        Ok(SpriteAtlas { name, entries })
    }

    /// The placement filed under a sprite's render-data key.
    pub fn placement(&self, key: &([u8; 16], i64)) -> Option<Placement> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, p)| *p)
    }
}
