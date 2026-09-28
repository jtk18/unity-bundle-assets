//! Byte-level builders for serialized files, bundles, and the objects inside them. Real game
//! files cannot ship with the crate, so the tests build their own.
//!
//! Object layouts are written out field by field from the engine's type trees for each
//! release, not generated from the reader's version gates, so a wrong gate fails a test
//! instead of agreeing with itself.

#![allow(dead_code)]

/// Unity's `TextureFormat` numbers, written out here rather than taken from the crate, so a
/// wrong constant there cannot agree with itself.
pub mod format {
    pub const ALPHA8: i32 = 1;
    pub const RGB24: i32 = 3;
    pub const RGBA32: i32 = 4;
    pub const ARGB32: i32 = 5;
    pub const DXT1: i32 = 10;
    pub const DXT5: i32 = 12;
    pub const BGRA32: i32 = 14;
    pub const BC7: i32 = 25;
}

pub const TEXTURE_2D: i32 = 28;
pub const SPRITE: i32 = 213;
pub const SPRITE_ATLAS: i32 = 687_078_895;

/// Object data in either byte order, aligned relative to the object's start.
pub struct W {
    pub buf: Vec<u8>,
    big: bool,
}

impl W {
    pub const fn new(big: bool) -> Self {
        Self {
            buf: Vec::new(),
            big,
        }
    }
    pub const fn le() -> Self {
        Self::new(false)
    }
    fn put<const N: usize>(&mut self, le: [u8; N], be: [u8; N]) -> &mut Self {
        self.buf.extend(if self.big { be } else { le });
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.put(v.to_le_bytes(), v.to_be_bytes())
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.buf.push(v as u8);
        self
    }
    pub fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend(b);
        self
    }
    pub fn align(&mut self) -> &mut Self {
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.i32(b.len() as i32);
        self.buf.extend(b);
        self.align()
    }
    pub fn string(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.buf.extend(std::iter::repeat_n(0, n));
        self
    }
    pub fn pptr(&mut self, file_id: i32, path_id: i64) -> &mut Self {
        self.i32(file_id).i64(path_id)
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        self.f32(x).f32(y).f32(w).f32(h)
    }
}

/// A texture's pixels: inline, or streamed from a file at an offset.
pub enum Pixels<'a> {
    Inline(&'a [u8]),
    Streamed {
        path: &'a str,
        offset: u64,
        size: u32,
    },
}

impl Pixels<'_> {
    const fn inline(&self) -> &[u8] {
        match self {
            Pixels::Inline(b) => b,
            Pixels::Streamed { .. } => &[],
        }
    }
    const fn stream(&self) -> (u64, u32, &str) {
        match *self {
            Pixels::Inline(_) => (0, 0, ""),
            Pixels::Streamed { path, offset, size } => (offset, size, path),
        }
    }
}

/// The engine generations whose `Texture2D` layouts differ.
#[derive(Clone, Copy, Debug)]
pub enum Layout {
    /// 5.6
    U5_6,
    /// 2017.1: 5.6's fields, three wrap modes.
    U2017_1,
    /// 2017.3 to 2018.1: plus the fallback fields.
    U2017_3,
    /// 2018.2 to 2019.3.0: fallback fields, streaming mipmaps.
    U2018_4,
    /// 2019.3.1 to 2019.4.8: plus m_IgnoreMasterTextureLimit.
    U2019_3,
    /// 2019.4.9: plus m_IsPreProcessed.
    U2019_4,
    /// 2020.1: plus m_MipsStripped; 64-bit stream offsets.
    U2020_1,
    /// 2020.2 to 2022.1: plus m_IsAlphaChannelOptional and m_PlatformBlob.
    U2021_3,
    /// 2022.2 to 2023.1: mips stripped, alpha optional, platform blob, limit group name.
    U2022_3,
    /// 2023.2 on (Unity 6): 2022.3 without the fallback fields.
    U6000,
}

/// A `Texture2D` object in `layout`.
#[allow(clippy::too_many_arguments)]
pub fn texture(
    layout: Layout,
    big: bool,
    name: &str,
    w: i32,
    h: i32,
    fmt: i32,
    px: &Pixels,
    platform_blob: &[u8],
) -> Vec<u8> {
    use Layout::*;
    let mut o = W::new(big);
    o.string(name);
    match layout {
        U5_6 | U2017_1 => {}
        U2017_3 | U2018_4 | U2019_3 | U2019_4 | U2020_1 => {
            o.i32(0).bool(false).align();
        }
        U2021_3 | U2022_3 => {
            o.i32(0).bool(false).bool(false).align();
        }
        U6000 => {
            o.bool(false).align(); // m_IsAlphaChannelOptional only
        }
    }
    o.i32(w).i32(h).i32(0);
    if matches!(layout, U2020_1 | U2021_3 | U2022_3 | U6000) {
        o.i32(0); // m_MipsStripped
    }
    o.i32(fmt).i32(1);
    match layout {
        U5_6 | U2017_1 | U2017_3 => {
            o.bool(false).align();
        }
        U2018_4 => {
            o.bool(false).bool(false).align().i32(0);
        }
        U2019_3 => {
            o.bool(false).bool(false).bool(false).align().i32(0);
        }
        U2019_4 | U2020_1 | U2021_3 => {
            o.bool(false)
                .bool(false)
                .bool(false)
                .bool(false)
                .align()
                .i32(0);
        }
        U2022_3 | U6000 => {
            o.bool(false).bool(false).bool(false).align();
            o.string(""); // m_MipmapLimitGroupName
            o.bool(false).align().i32(0);
        }
    }
    o.i32(1).i32(2);
    o.zeros(if matches!(layout, U5_6) { 16 } else { 24 });
    o.i32(0).i32(1);
    if matches!(layout, U2021_3 | U2022_3 | U6000) {
        o.bytes(platform_blob);
    }
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    if matches!(layout, U2020_1 | U2021_3 | U2022_3 | U6000) {
        o.u64(offset);
    } else {
        o.u32(offset as u32);
    }
    o.u32(size).string(path);
    o.buf
}

/// A sprite mesh: positions in sprite units, and index triples.
#[derive(Clone, Copy)]
pub struct Mesh<'a> {
    pub vertices: &'a [[f32; 2]],
    pub indices: &'a [u16],
    /// Which vertex stream holds positions (the other holds UVs).
    pub pos_stream: u8,
    /// Written as the sub-mesh's base vertex, with that many unused vertices first.
    pub base_vertex: u32,
    /// A second sub-mesh of lines (topology 3) over `junk_triangle`, which must be ignored.
    pub junk_lines: bool,
    /// The position channel's vertex format: 0 float32, 1 float16.
    pub pos_format: u8,
}

impl Mesh<'static> {
    pub const BASE: Self = Mesh {
        vertices: &[],
        indices: &[],
        pos_stream: 0,
        base_vertex: 0,
        junk_lines: false,
        pos_format: 0,
    };
}

/// A `Sprite` object, Unity 2019.1 to 6000.4 layout (`is_polygon`), or 6000.5 (without).
#[allow(clippy::too_many_arguments)]
pub fn sprite(
    big: bool,
    unity_6000_5: bool,
    name: &str,
    rect: [f32; 4],
    pivot: [f32; 2],
    key: i64,
    atlas: i64,
    texture: i64,
    alpha_texture: i64,
    texture_rect: [f32; 4],
    settings: u32,
    downscale: f32,
    mesh: &Mesh,
) -> Vec<u8> {
    sprite_full(
        big,
        unity_6000_5,
        name,
        rect,
        pivot,
        key,
        atlas,
        texture,
        alpha_texture,
        texture_rect,
        settings,
        downscale,
        mesh,
        1.0,
        [0.0, 0.0],
    )
}

/// [`sprite`] with pixels per unit and the texture rect's offset within the full rect.
#[allow(clippy::too_many_arguments)]
pub fn sprite_full(
    big: bool,
    unity_6000_5: bool,
    name: &str,
    rect: [f32; 4],
    pivot: [f32; 2],
    key: i64,
    atlas: i64,
    texture: i64,
    alpha_texture: i64,
    texture_rect: [f32; 4],
    settings: u32,
    downscale: f32,
    mesh: &Mesh,
    pixels_per_unit: f32,
    rect_offset: [f32; 2],
) -> Vec<u8> {
    let mut o = W::new(big);
    o.string(name);
    o.rect(rect[0], rect[1], rect[2], rect[3]);
    o.zeros(8 + 16); // m_Offset, m_Border
    o.f32(pixels_per_unit);
    o.f32(pivot[0]).f32(pivot[1]);
    o.u32(1); // m_Extrude
    if !unity_6000_5 {
        o.bool(true); // m_IsPolygon
    }
    o.align();
    o.raw(&[7; 16]).i64(key);
    o.i32(0); // m_AtlasTags
    o.pptr(0, atlas);
    // m_RD
    o.pptr(0, texture).pptr(0, alpha_texture);
    o.i32(0); // secondaryTextures
    let b = mesh.base_vertex as usize;
    // Unused vertices before the mesh's own, reached only through the base vertex; and, for
    // `junk_lines`, a huge triangle after them.
    let mut verts: Vec<[f32; 2]> = vec![[9e3, 9e3]; b];
    verts.extend_from_slice(mesh.vertices);
    let junk = verts.len() as u16;
    if mesh.junk_lines {
        verts.extend([[-1e4, -1e4], [1e4, -1e4], [0.0, 1e4]]);
    }
    let index_count = mesh.indices.len() as u32;
    let mut ib = W::new(big);
    for &i in mesh.indices {
        ib.u16(i);
    }
    let junk_at = ib.buf.len() as u32;
    if mesh.junk_lines {
        for i in [0u16, 1, 2] {
            ib.u16(junk + i - b as u16);
        }
    }
    let submeshes = u32::from(index_count > 0) + u32::from(mesh.junk_lines);
    o.u32(submeshes);
    if index_count > 0 {
        o.u32(0)
            .u32(index_count)
            .i32(0)
            .u32(mesh.base_vertex)
            .u32(0);
        o.u32(verts.len() as u32).zeros(24);
    }
    if mesh.junk_lines {
        o.u32(junk_at).u32(3).i32(3).u32(mesh.base_vertex).u32(0);
        o.u32(verts.len() as u32).zeros(24);
    }
    o.bytes(&ib.buf);
    o.u32(verts.len() as u32);
    // Channel 0 is position (3 components), channel 1 UV (2 floats), in the streams asked.
    let (ps, us) = if mesh.pos_stream == 0 {
        (0u8, 1u8)
    } else {
        (1, 0)
    };
    o.i32(2)
        .raw(&[ps, 0, mesh.pos_format, 3])
        .raw(&[us, 0, 0, 2]);
    let mut vb = W::new(big);
    let positions = |vb: &mut W| {
        for v in &verts {
            match mesh.pos_format {
                0 => {
                    vb.f32(v[0]).f32(v[1]).f32(0.0);
                }
                10 => {
                    vb.u32(v[0] as u32).u32(v[1] as u32).u32(0);
                }
                _ => {
                    vb.u16(0).u16(0).u16(0);
                }
            }
        }
    };
    let uvs = |vb: &mut W| {
        for _ in &verts {
            vb.f32(0.0).f32(0.0);
        }
    };
    let pad = |vb: &mut W| {
        while vb.buf.len() % 16 != 0 {
            vb.buf.push(0);
        }
    };
    if ps == 0 {
        positions(&mut vb);
        pad(&mut vb);
        uvs(&mut vb);
    } else {
        uvs(&mut vb);
        pad(&mut vb);
        positions(&mut vb);
    }
    o.bytes(&vb.buf);
    o.i32(0); // m_Bindpose
    o.rect(
        texture_rect[0],
        texture_rect[1],
        texture_rect[2],
        texture_rect[3],
    );
    o.f32(rect_offset[0]).f32(rect_offset[1]); // textureRectOffset
    o.f32(0.0).f32(0.0); // atlasRectOffset
    o.u32(settings);
    o.zeros(16); // uvTransform
    o.f32(downscale);
    o.buf
}

/// A `SpriteAtlas` object (2020.2 on) with one entry per `(key, texture, rect, settings)`.
pub fn atlas(big: bool, entries: &[(i64, i64, [f32; 4], u32, f32)]) -> Vec<u8> {
    atlas_with(big, true, entries)
}

/// A `SpriteAtlas` object; `secondary` for the 2020.2 layout, whose entries end in a list of
/// secondary textures.
pub fn atlas_with(
    big: bool,
    secondary: bool,
    entries: &[(i64, i64, [f32; 4], u32, f32)],
) -> Vec<u8> {
    let mut o = W::new(big);
    o.string("atlas");
    o.i32(0); // m_PackedSprites
    o.i32(0); // m_PackedSpriteNamesToIndex
    o.i32(entries.len() as i32);
    for &(key, texture, r, settings, downscale) in entries {
        o.raw(&[7; 16]).i64(key);
        o.pptr(0, texture).pptr(0, 0);
        o.rect(r[0], r[1], r[2], r[3]);
        o.f32(0.0).f32(0.0).f32(0.0).f32(0.0); // textureRectOffset, atlasRectOffset
        o.zeros(16); // uvTransform
        o.f32(downscale);
        o.u32(settings);
        if secondary {
            o.i32(0); // secondaryTextures
        }
    }
    o.string("tag"); // m_Tag
    o.bool(false).align(); // m_IsVariant
    o.buf
}

/// What else a serialized file's metadata carries.
#[derive(Default)]
pub struct Extras {
    /// Type trees for every type (one node each), with type-tree dependencies from v21.
    pub type_trees: bool,
    /// A MonoBehaviour type, whose entry carries a script ID.
    pub mono: bool,
    pub scripts: usize,
    pub externals: Vec<String>,
}

/// A serialized file holding `objects` as `(path_id, class_id, data)`, no type trees.
pub fn serialized(
    version: u32,
    unity: &str,
    big: bool,
    platform: i32,
    objects: &[(i64, i32, Vec<u8>)],
) -> Vec<u8> {
    serialized_with(version, unity, big, platform, objects, &Extras::default())
}

/// [`serialized`] with the metadata sections the reader must step over.
pub fn serialized_with(
    version: u32,
    unity: &str,
    big: bool,
    platform: i32,
    objects: &[(i64, i32, Vec<u8>)],
    extras: &Extras,
) -> Vec<u8> {
    let header_len = if version >= 22 { 48 } else { 20 };
    let mut classes: Vec<i32> = Vec::new();
    for (_, c, _) in objects {
        if !classes.contains(c) {
            classes.push(*c);
        }
    }
    if extras.mono {
        classes.push(114);
    }
    let mut m = W::new(big);
    m.raw(unity.as_bytes()).raw(&[0]);
    m.i32(platform);
    m.bool(extras.type_trees);
    m.i32(classes.len() as i32);
    for c in &classes {
        m.i32(*c).bool(false).u16(0xffff);
        if *c == 114 {
            m.raw(&[0x5c; 16]); // script ID
        }
        m.raw(&[0xab; 16]); // type hash
        if extras.type_trees {
            let node = if version >= 19 { 32 } else { 24 };
            m.i32(2).i32(8); // two nodes, eight bytes of strings
            m.raw(&vec![0x3c; 2 * node]).raw(b"Base\0ab\0");
            if version >= 21 {
                m.i32(2).i32(5).i32(6); // two dependencies
            }
        }
    }
    m.i32(objects.len() as i32);
    let mut start = 0u64;
    let mut starts = Vec::new();
    for (path_id, class, data) in objects {
        while (header_len + m.buf.len()) % 4 != 0 {
            m.buf.push(0);
        }
        m.i64(*path_id);
        if version >= 22 {
            m.u64(start);
        } else {
            m.u32(start as u32);
        }
        m.u32(data.len() as u32);
        m.i32(classes.iter().position(|c| c == class).unwrap() as i32);
        starts.push(start);
        start = (start + data.len() as u64).div_ceil(8) * 8;
    }
    m.i32(extras.scripts as i32);
    for i in 0..extras.scripts {
        m.i32(i as i32);
        while (header_len + m.buf.len()) % 4 != 0 {
            m.buf.push(0);
        }
        m.i64(1000 + i as i64);
    }
    m.i32(extras.externals.len() as i32);
    for path in &extras.externals {
        m.raw(&[0])
            .raw(&[0x9e; 16])
            .i32(2)
            .raw(path.as_bytes())
            .raw(&[0]);
    }
    let meta = m.buf;

    let data_offset = (header_len + meta.len()).div_ceil(16) * 16;
    let file_size = data_offset + start as usize;
    // From v22 the 32-bit fields are left 0 and the 64-bit ones below carry the values.
    let small = |v: usize| if version >= 22 { 0 } else { v as u32 };
    let mut out = Vec::new();
    out.extend(small(meta.len()).to_be_bytes());
    out.extend(small(file_size).to_be_bytes());
    out.extend(version.to_be_bytes());
    out.extend(small(data_offset).to_be_bytes());
    out.extend([big as u8, 0, 0, 0]);
    if version >= 22 {
        out.extend((meta.len() as u32).to_be_bytes());
        out.extend((file_size as u64).to_be_bytes());
        out.extend((data_offset as u64).to_be_bytes());
        out.extend([0; 8]);
    }
    out.extend(meta);
    out.resize(data_offset, 0);
    for ((_, _, data), s) in objects.iter().zip(starts) {
        out.resize(data_offset + s as usize, 0);
        out.extend(data);
    }
    out.resize(file_size, 0);
    out
}

/// An LZ4 block of one literal run.
pub fn lz4_literals(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let n = data.len();
    out.push((n.min(15) as u8) << 4);
    if n >= 15 {
        let mut rest = n - 15;
        while rest >= 255 {
            out.push(255);
            rest -= 255;
        }
        out.push(rest as u8);
    }
    out.extend(data);
    out
}

/// An LZ4 block with real matches (greedy, naive search), so readers see the match path.
pub fn lz4_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut anchor = 0;
    let push_len = |out: &mut Vec<u8>, mut n: usize| {
        while n >= 255 {
            out.push(255);
            n -= 255;
        }
        out.push(n as u8);
    };
    // The last five bytes are always literals, as the format requires.
    while i + 12 <= data.len() {
        let mut best = (0, 0);
        let lo = i.saturating_sub(65535);
        for j in lo..i {
            let mut n = 0;
            while i + n + 5 < data.len() && data[j + n] == data[i + n] {
                n += 1;
            }
            if n >= 4 && n > best.1 {
                best = (i - j, n);
            }
        }
        if best.1 < 4 {
            i += 1;
            continue;
        }
        let (offset, len) = best;
        let lits = i - anchor;
        let ml = len - 4;
        out.push(((lits.min(15) as u8) << 4) | ml.min(15) as u8);
        if lits >= 15 {
            push_len(&mut out, lits - 15);
        }
        out.extend(&data[anchor..i]);
        out.extend((offset as u16).to_le_bytes());
        if ml >= 15 {
            push_len(&mut out, ml - 15);
        }
        i += len;
        anchor = i;
    }
    let lits = data.len() - anchor;
    out.push((lits.min(15) as u8) << 4);
    if lits >= 15 {
        push_len(&mut out, lits - 15);
    }
    out.extend(&data[anchor..]);
    out
}

/// Unity's LZMA framing: properties and dictionary size, then the stream, no length field.
pub fn lzma(data: &[u8]) -> Vec<u8> {
    use lzma_rs::compress::{Options, UnpackedSize};
    let mut out = Vec::new();
    let options = Options {
        unpacked_size: UnpackedSize::SkipWritingToHeader,
    };
    lzma_rs::lzma_compress_with_options(&mut &data[..], &mut out, &options).unwrap();
    out
}

/// How to build a bundle.
#[derive(Clone)]
pub struct BundleOpts {
    pub format: u32,
    pub revision: String,
    /// Compression of each block: 0 stored, 1 LZMA, 2 LZ4, 3 LZ4HC. The stream is split into
    /// one block per entry here.
    pub blocks: Vec<u16>,
    /// Compression of the directory: 0, 1 or 2.
    pub info: u32,
    pub info_at_end: bool,
    /// Set flag 0x200 and pad before the blocks, as newer engines do.
    pub padding_flag: bool,
    pub extra_flags: u32,
    /// Pad the header to 16 bytes even at format 6, as 2019.4.15 and later do.
    pub align_header: bool,
}

impl BundleOpts {
    pub fn new(format: u32, revision: &str) -> Self {
        Self {
            format,
            revision: revision.into(),
            blocks: vec![2, 1],
            info: 0,
            info_at_end: false,
            padding_flag: false,
            extra_flags: 0,
            align_header: false,
        }
    }
}

fn compress(kind: u16, data: &[u8]) -> Vec<u8> {
    match kind {
        0 => data.to_vec(),
        1 => lzma(data),
        4 => lz4_compress(data),
        _ => lz4_literals(data),
    }
}

/// A UnityFS bundle holding `entries` as `(path, data, flags)`.
pub fn bundle(opts: &BundleOpts, entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let stream: Vec<u8> = entries.iter().flat_map(|e| e.1.iter().copied()).collect();
    // Split the stream into as many blocks as `opts.blocks` names, evenly.
    let n = opts.blocks.len().max(1);
    let chunk = stream.len().div_ceil(n).max(1);
    let mut blocks = Vec::new();
    for (i, part) in stream.chunks(chunk).enumerate() {
        let kind = opts.blocks[i.min(opts.blocks.len() - 1)];
        // Kind 4 is LZ4 with matches, written as the LZ4 flag.
        blocks.push((
            part.len(),
            compress(kind, part),
            if kind == 4 { 2 } else { kind },
        ));
    }
    let mut info = vec![0u8; 16];
    info.extend((blocks.len() as i32).to_be_bytes());
    for (size, data, kind) in &blocks {
        info.extend((*size as u32).to_be_bytes());
        info.extend((data.len() as u32).to_be_bytes());
        info.extend(kind.to_be_bytes());
    }
    info.extend((entries.len() as i32).to_be_bytes());
    let mut offset = 0i64;
    for (path, data, flags) in entries {
        info.extend(offset.to_be_bytes());
        info.extend((data.len() as i64).to_be_bytes());
        info.extend(flags.to_be_bytes());
        info.extend(path.as_bytes());
        info.push(0);
        offset += data.len() as i64;
    }
    let info_c = compress(opts.info as u16, &info);

    let mut flags = 0x40 | opts.info | opts.extra_flags;
    if opts.info_at_end {
        flags |= 0x80;
    }
    if opts.padding_flag {
        flags |= 0x200;
    }
    let mut out = b"UnityFS\0".to_vec();
    out.extend(opts.format.to_be_bytes());
    out.extend(b"5.x.x\0");
    out.extend(opts.revision.as_bytes());
    out.push(0);
    out.extend(0i64.to_be_bytes());
    out.extend((info_c.len() as u32).to_be_bytes());
    out.extend((info.len() as u32).to_be_bytes());
    out.extend(flags.to_be_bytes());
    if opts.format >= 7 || opts.align_header {
        out.resize(out.len().div_ceil(16) * 16, 0);
    }
    if !opts.info_at_end {
        out.extend(&info_c);
    }
    if opts.padding_flag {
        out.resize(out.len().div_ceil(16) * 16, 0);
    }
    for (_, data, _) in &blocks {
        out.extend(data);
    }
    if opts.info_at_end {
        out.extend(&info_c);
    }
    out
}

/// A 4x4 RGBA32 image with every byte distinct, as stored (bottom row first).
pub fn rgba_4x4() -> Vec<u8> {
    (0..64).collect()
}

/// Reverse the rows of an RGBA image.
pub fn flip(rgba: &[u8], width: usize) -> Vec<u8> {
    rgba.chunks(width * 4).rev().flatten().copied().collect()
}

/// A temporary directory removed when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "unity-bundle-assets-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn file(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
