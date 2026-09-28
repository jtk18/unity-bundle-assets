//! End to end on files built here, byte by byte: serialized files holding textures in three
//! engine generations' layouts, and bundles holding those files with LZ4 and LZMA blocks and
//! a streamed `.resS`. Real game files cannot ship with the crate, so these stand in.
//!
//! The Texture2D layouts below are written out field by field from the engine's own type
//! trees for each release, not generated from the reader's version gates, so a wrong gate
//! fails here rather than agreeing with itself.

use unity_bundle_assets::{decode::format, is_bundle, Assets, Bundle, Error, SerializedFile};

/// Little-endian object data, aligned relative to the object's start.
#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn bool(&mut self, v: bool) -> &mut Self {
        self.0.push(v as u8);
        self
    }
    fn align(&mut self) -> &mut Self {
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.i32(b.len() as i32);
        self.0.extend(b);
        self.align()
    }
    fn string(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.extend(std::iter::repeat_n(0, n));
        self
    }
}

/// A texture's pixels: inline, or streamed from a file at an offset.
enum Pixels<'a> {
    Inline(&'a [u8]),
    Streamed {
        path: &'a str,
        offset: u64,
        size: u32,
    },
}

impl Pixels<'_> {
    fn inline(&self) -> &[u8] {
        match self {
            Pixels::Inline(b) => b,
            Pixels::Streamed { .. } => &[],
        }
    }
    fn stream(&self) -> (u64, u32, &str) {
        match *self {
            Pixels::Inline(_) => (0, 0, ""),
            Pixels::Streamed { path, offset, size } => (offset, size, path),
        }
    }
}

/// `Texture2D` as Unity 5.6 writes it.
fn texture_5_6(name: &str, w: i32, h: i32, fmt: i32, px: &Pixels) -> Vec<u8> {
    let mut o = W::default();
    o.string(name).i32(w).i32(h).i32(0).i32(fmt).i32(1);
    o.bool(false).align(); // m_IsReadable
    o.i32(1).i32(2); // m_ImageCount, m_TextureDimension
    o.zeros(16); // m_TextureSettings, one wrap mode
    o.i32(0).i32(1); // m_LightmapFormat, m_ColorSpace
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    o.u32(offset as u32).u32(size).string(path);
    o.0
}

/// `Texture2D` as Unity 2018.4 writes it.
fn texture_2018_4(name: &str, w: i32, h: i32, fmt: i32, px: &Pixels) -> Vec<u8> {
    let mut o = W::default();
    o.string(name);
    o.i32(0).bool(false).align(); // m_ForcedFallbackFormat, m_DownscaleFallback
    o.i32(w).i32(h).i32(0).i32(fmt).i32(1);
    o.bool(false).bool(false).align(); // m_IsReadable, m_StreamingMipmaps
    o.i32(0); // m_StreamingMipmapsPriority
    o.i32(1).i32(2);
    o.zeros(24); // m_TextureSettings, wrap U/V/W
    o.i32(0).i32(1);
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    o.u32(offset as u32).u32(size).string(path);
    o.0
}

/// `Texture2D` as Unity 2017.1 writes it: 5.6's fields, three wrap modes.
fn texture_2017_1(name: &str, w: i32, h: i32, fmt: i32, px: &Pixels) -> Vec<u8> {
    let mut o = W::default();
    o.string(name).i32(w).i32(h).i32(0).i32(fmt).i32(1);
    o.bool(false).align();
    o.i32(1).i32(2);
    o.zeros(24);
    o.i32(0).i32(1);
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    o.u32(offset as u32).u32(size).string(path);
    o.0
}

/// `Texture2D` as Unity 2019.3.5 to 2019.4.8 write it: 2018.4's, plus
/// `m_IgnoreMasterTextureLimit`; from 2019.4.9 also `m_IsPreProcessed`.
fn texture_2019_4(
    name: &str,
    w: i32,
    h: i32,
    fmt: i32,
    px: &Pixels,
    preprocessed: bool,
) -> Vec<u8> {
    let mut o = W::default();
    o.string(name);
    o.i32(0).bool(false).align();
    o.i32(w).i32(h).i32(0).i32(fmt).i32(1);
    o.bool(false).bool(false); // m_IsReadable, m_IgnoreMasterTextureLimit
    if preprocessed {
        o.bool(false); // m_IsPreProcessed
    }
    o.bool(false).align(); // m_StreamingMipmaps
    o.i32(0);
    o.i32(1).i32(2);
    o.zeros(24);
    o.i32(0).i32(1);
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    o.u32(offset as u32).u32(size).string(path);
    o.0
}

/// `Texture2D` as Unity 2022.3 writes it.
fn texture_2022_3(name: &str, w: i32, h: i32, fmt: i32, px: &Pixels) -> Vec<u8> {
    let mut o = W::default();
    o.string(name);
    o.i32(0).bool(false).bool(false).align(); // fallback, downscale, alpha optional
    o.i32(w).i32(h).i32(0).i32(0).i32(fmt).i32(1); // .., m_MipsStripped, format, mips
    o.bool(false).bool(false).bool(false).align(); // readable, preprocessed, ignore limit
    o.string(""); // m_MipmapLimitGroupName
    o.bool(false).align().i32(0); // m_StreamingMipmaps, priority
    o.i32(1).i32(2);
    o.zeros(24);
    o.i32(0).i32(1);
    o.bytes(&[]); // m_PlatformBlob
    o.bytes(px.inline());
    let (offset, size, path) = px.stream();
    o.u64(offset).u32(size).string(path);
    o.0
}

/// A serialized file with one Texture2D object (path ID 7), little-endian, no type trees.
fn serialized(version: u32, unity: &str, object: &[u8]) -> Vec<u8> {
    let header_len = if version >= 22 { 48 } else { 20 };
    let mut meta = Vec::new();
    meta.extend(unity.as_bytes());
    meta.push(0);
    meta.extend(19i32.to_le_bytes()); // target platform
    meta.push(0); // no type trees
    meta.extend(1i32.to_le_bytes()); // one type
    meta.extend(28i32.to_le_bytes()); // Texture2D
    meta.push(0); // stripped
    meta.extend((-1i16).to_le_bytes()); // script type index
    meta.extend([0; 16]); // type hash
    meta.extend(1i32.to_le_bytes()); // one object
    while (header_len + meta.len()) % 4 != 0 {
        meta.push(0);
    }
    meta.extend(7i64.to_le_bytes()); // path ID
    if version >= 22 {
        meta.extend(0u64.to_le_bytes());
    } else {
        meta.extend(0u32.to_le_bytes());
    }
    meta.extend((object.len() as u32).to_le_bytes());
    meta.extend(0i32.to_le_bytes()); // type index
    meta.extend(0i32.to_le_bytes()); // scripts
    meta.extend(0i32.to_le_bytes()); // externals

    let mut data_offset = header_len + meta.len();
    data_offset = data_offset.div_ceil(16) * 16;
    let file_size = data_offset + object.len();
    let mut out = Vec::new();
    out.extend((meta.len() as u32).to_be_bytes());
    out.extend((file_size as u32).to_be_bytes());
    out.extend(version.to_be_bytes());
    out.extend((data_offset as u32).to_be_bytes());
    out.extend([0, 0, 0, 0]); // little-endian, reserved
    if version >= 22 {
        out.extend((meta.len() as u32).to_be_bytes());
        out.extend((file_size as u64).to_be_bytes());
        out.extend((data_offset as u64).to_be_bytes());
        out.extend([0; 8]);
    }
    out.extend(meta);
    out.resize(data_offset, 0);
    out.extend(object);
    out
}

/// An LZ4 block of one literal run: valid, and all a decoder needs to prove it reads runs.
fn lz4_literals(data: &[u8]) -> Vec<u8> {
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

/// Unity's LZMA framing: properties and dictionary size, then the stream, no length field.
fn lzma(data: &[u8]) -> Vec<u8> {
    use lzma_rs::compress::{Options, UnpackedSize};
    let mut out = Vec::new();
    let options = Options {
        unpacked_size: UnpackedSize::SkipWritingToHeader,
    };
    lzma_rs::lzma_compress_with_options(&mut &data[..], &mut out, &options).unwrap();
    out
}

/// A UnityFS bundle holding `entries`, the stream split into an LZ4 block and an LZMA
/// block. `format` 7 and later align the header to 16 bytes.
fn bundle(format: u32, revision: &str, entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let stream: Vec<u8> = entries.iter().flat_map(|e| e.1.iter().copied()).collect();
    let split = stream.len() / 2;
    let blocks = [
        (split, lz4_literals(&stream[..split]), 2u16),
        (stream.len() - split, lzma(&stream[split..]), 1u16),
    ];
    let mut info = vec![0u8; 16];
    info.extend((blocks.len() as i32).to_be_bytes());
    for (size, data, flags) in &blocks {
        info.extend((*size as u32).to_be_bytes());
        info.extend((data.len() as u32).to_be_bytes());
        info.extend(flags.to_be_bytes());
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

    let mut out = b"UnityFS\0".to_vec();
    out.extend(format.to_be_bytes());
    out.extend(b"5.x.x\0");
    out.extend(revision.as_bytes());
    out.push(0);
    out.extend(0i64.to_be_bytes()); // total size; not checked
    out.extend((info.len() as u32).to_be_bytes());
    out.extend((info.len() as u32).to_be_bytes());
    out.extend(0x40u32.to_be_bytes()); // directory with blocks, info uncompressed
    if format >= 7 {
        out.resize(out.len().div_ceil(16) * 16, 0);
    }
    out.extend(info);
    for (_, data, _) in &blocks {
        out.extend(data);
    }
    out
}

/// A 4x4 RGBA32 image with every byte distinct.
fn rgba_4x4() -> Vec<u8> {
    (0..64).collect()
}

fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("unity-bundle-assets-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn inline_textures_in_every_layout() {
    let pixels = rgba_4x4();
    let cases = [
        (
            17,
            "5.6.6f2",
            texture_5_6("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
        ),
        (
            17,
            "2018.4.36f1",
            texture_2018_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
        ),
        (
            22,
            "2022.3.62f1",
            texture_2022_3("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
        ),
        // Either side of each gate the engine moved.
        (
            17,
            "2017.1.0f1",
            texture_2017_1("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
        ),
        (
            21,
            "2019.3.0f1",
            texture_2018_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
        ),
        (
            21,
            "2019.3.5f1",
            texture_2019_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels), false),
        ),
        (
            21,
            "2019.4.8f1",
            texture_2019_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels), false),
        ),
        (
            21,
            "2019.4.40f1",
            texture_2019_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels), true),
        ),
    ];
    for (version, unity, object) in cases {
        let path = temp_file(
            &format!("inline-{unity}.assets"),
            &serialized(version, unity, &object),
        );
        let assets = Assets::open(&path).unwrap();
        let listed = assets.textures(|_| true);
        assert_eq!(listed.len(), 1, "{unity}");
        assert_eq!((listed[0].path_id, listed[0].name.as_str()), (7, "t"));
        let meta = assets.texture(7).unwrap();
        assert_eq!(
            (meta.width, meta.height, meta.format),
            (4, 4, format::RGBA32),
            "{unity}"
        );
        let image = assets.decode_texture(7).unwrap();
        assert_eq!((image.width, image.height), (4, 4));
        assert_eq!(image.rgba, pixels, "{unity}");
    }
}

#[test]
fn a_wrong_layout_is_caught_not_misread() {
    // A 2018.4 object labelled 5.6: the reader must not produce plausible nonsense.
    let pixels = rgba_4x4();
    let object = texture_2018_4("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels));
    let path = temp_file("mislabelled.assets", &serialized(17, "5.6.6f2", &object));
    let assets = Assets::open(&path).unwrap();
    assert!(assets
        .decode_texture(7)
        .map(|i| i.rgba != pixels)
        .unwrap_or(true));
}

#[test]
fn bundles_with_lz4_and_lzma_blocks_and_a_streamed_texture() {
    // RGB24 pixels streamed from the bundle's .resS, after 5 bytes of something else.
    let rgb: Vec<u8> = (100..148).collect();
    let mut ress = vec![9; 5];
    ress.extend(&rgb);
    let stream = |_| Pixels::Streamed {
        path: "archive:/CAB-test/CAB-test.resS",
        offset: 5,
        size: rgb.len() as u32,
    };
    let cases = [
        (
            6,
            "5.6.6f2",
            17,
            texture_5_6("s", 4, 4, format::RGB24, &stream(())),
        ),
        (
            6,
            "2018.4.36f1",
            17,
            texture_2018_4("s", 4, 4, format::RGB24, &stream(())),
        ),
        (
            7,
            "2022.3.62f1",
            22,
            texture_2022_3("s", 4, 4, format::RGB24, &stream(())),
        ),
    ];
    for (bundle_format, unity, version, object) in cases {
        let file = serialized(version, unity, &object);
        let bytes = bundle(
            bundle_format,
            unity,
            &[("CAB-test", &file, 4), ("CAB-test.resS", &ress, 0)],
        );
        assert!(is_bundle(&bytes));
        let parsed = Bundle::parse(&bytes).unwrap();
        assert_eq!(parsed.serialized_files().count(), 1, "{unity}");

        let path = temp_file(&format!("bundle-{unity}"), &bytes);
        let assets = Assets::open(&path).unwrap();
        let image = assets.decode_texture(7).unwrap();
        let want: Vec<u8> = rgb
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        assert_eq!(image.rgba, want, "{unity}");
    }
}

#[test]
fn bad_input_is_an_error() {
    let pixels = rgba_4x4();
    let file = serialized(
        17,
        "5.6.6f2",
        &texture_5_6("t", 4, 4, format::RGBA32, &Pixels::Inline(&pixels)),
    );
    let bytes = bundle(6, "5.6.6f2", &[("CAB-t", &file, 4)]);

    // A bundle handed to the serialized-file parser says what it is.
    match SerializedFile::parse(bytes.clone()) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("asset bundle"), "{msg}"),
        other => panic!("expected Unsupported, got {:?}", other.map(|_| ())),
    }
    // Every truncation fails cleanly.
    for n in 0..bytes.len() {
        assert!(
            Bundle::parse(&bytes[..n]).is_err(),
            "truncated to {n} parsed"
        );
    }
    for n in 0..file.len() {
        let _ = SerializedFile::parse(file[..n].to_vec());
    }
    // An empty texture, as dynamic fonts store.
    let empty = texture_5_6("Font Texture", 0, 0, format::ALPHA8, &Pixels::Inline(&[]));
    let path = temp_file("empty.assets", &serialized(17, "5.6.6f2", &empty));
    let err = Assets::open(&path).unwrap().decode_texture(7).unwrap_err();
    assert!(err.to_string().contains("empty"), "{err}");
}
