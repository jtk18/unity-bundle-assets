//! Textures and bundles end to end, on files built byte by byte (see `common`).

mod common;
use common::*;
use unity_bundle_assets::{is_bundle, Assets, Bundle, Error, SerializedFile};

fn inline(layout: Layout, pixels: &[u8]) -> Vec<u8> {
    texture(
        layout,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(pixels),
        &[],
    )
}

#[test]
fn inline_textures_in_every_layout() {
    let pixels = rgba_4x4();
    let dir = TempDir::new("inline");
    let cases = [
        (17, "5.6.6f2", Layout::U5_6),
        (17, "2017.1.0f1", Layout::U2017_1),
        (17, "2018.4.36f1", Layout::U2018_4),
        (21, "2019.3.0f1", Layout::U2018_4),
        (21, "2019.3.5f1", Layout::U2019_3),
        (21, "2019.4.8f1", Layout::U2019_3),
        (21, "2019.4.40f1", Layout::U2019_4),
        (22, "2022.3.62f1", Layout::U2022_3),
        (22, "2023.2.20f1", Layout::U6000),
        (22, "6000.0.23f1", Layout::U6000),
        (22, "6000.5.0a5", Layout::U6000),
    ];
    for (version, unity, layout) in cases {
        let file = serialized(
            version,
            unity,
            false,
            19,
            &[(7, TEXTURE_2D, inline(layout, &pixels))],
        );
        let path = dir.file(&format!("inline-{unity}.assets"), &file);
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
        assert_eq!(image.rgba, flip(&pixels, 4), "{unity}: top row first");
    }
}

#[test]
fn newer_engines_than_known_are_refused() {
    let file = serialized(
        22,
        "6000.6.0f1",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U6000, &rgba_4x4()))],
    );
    let assets = Assets::open(TempDir::new("new").file("t.assets", &file)).unwrap();
    match assets.texture(7) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("newer"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_wrong_layout_is_caught_not_misread() {
    // A 2018.4 object labelled 5.6: the reader must not produce plausible nonsense.
    let pixels = rgba_4x4();
    let file = serialized(
        17,
        "5.6.6f2",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U2018_4, &pixels))],
    );
    let assets = Assets::open(TempDir::new("wrong").file("t.assets", &file)).unwrap();
    assert!(assets
        .decode_texture(7)
        .map(|i| i.rgba != flip(&pixels, 4))
        .unwrap_or(true));
}

#[test]
fn big_endian_files() {
    let pixels = rgba_4x4();
    let obj = texture(
        Layout::U2018_4,
        true,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&pixels),
        &[],
    );
    let file = serialized(17, "2018.4.36f1", true, 19, &[(7, TEXTURE_2D, obj)]);
    let assets = Assets::open(TempDir::new("be").file("t.assets", &file)).unwrap();
    assert!(assets.file().big_endian());
    assert_eq!(assets.decode_texture(7).unwrap().rgba, flip(&pixels, 4));
}

#[test]
fn several_objects_and_duplicate_ids() {
    let a = rgba_4x4();
    let b: Vec<u8> = (100..164).collect();
    let file = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[
            (5, TEXTURE_2D, inline(Layout::U2018_4, &a)),
            (9, TEXTURE_2D, inline(Layout::U2018_4, &b)),
            (5, TEXTURE_2D, inline(Layout::U2018_4, &b)), // repeated ID: the first wins
        ],
    );
    let assets = Assets::open(TempDir::new("dup").file("t.assets", &file)).unwrap();
    assert_eq!(assets.decode_texture(9).unwrap().rgba, flip(&b, 4));
    assert_eq!(assets.decode_texture(5).unwrap().rgba, flip(&a, 4));
    assert!(matches!(assets.decode_texture(6), Err(Error::NotFound(_))));
}

#[test]
fn mip_levels_after_the_first_are_ignored() {
    let mut pixels = rgba_4x4();
    pixels.extend([9u8; 16]); // a 2x2 mip
    let file = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U2018_4, &pixels))],
    );
    let assets = Assets::open(TempDir::new("mip").file("t.assets", &file)).unwrap();
    assert_eq!(assets.decode_texture(7).unwrap().rgba, flip(&rgba_4x4(), 4));
}

#[test]
fn streamed_textures_beside_the_file() {
    let dir = TempDir::new("stream");
    let pixels = rgba_4x4();
    let mut ress = vec![1, 2, 3];
    ress.extend(&pixels);
    dir.file("t.assets.resS", &ress);
    let px = Pixels::Streamed {
        path: "t.assets.resS",
        offset: 3,
        size: 64,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::RGBA32, &px, &[]);
    let path = dir.file(
        "t.assets",
        &serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]),
    );
    assert_eq!(
        Assets::open(&path).unwrap().decode_texture(7).unwrap().rgba,
        flip(&pixels, 4)
    );
}

fn texture_file(unity: &str, version: u32, layout: Layout, px: &Pixels) -> Vec<u8> {
    let obj = texture(layout, false, "s", 4, 4, format::RGB24, px, &[]);
    serialized(version, unity, false, 19, &[(7, TEXTURE_2D, obj)])
}

#[test]
fn bundles_in_every_container_variant() {
    // RGB24 pixels streamed from the bundle's .resS, after 5 bytes of something else.
    let rgb: Vec<u8> = (100..148).collect();
    let mut ress = vec![9; 5];
    ress.extend(&rgb);
    let px = Pixels::Streamed {
        path: "archive:/CAB-test/CAB-test.resS",
        offset: 5,
        size: rgb.len() as u32,
    };
    let want: Vec<u8> = flip(
        &rgb.chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect::<Vec<_>>(),
        4,
    );

    let old = |f: fn(&mut BundleOpts)| {
        let mut o = BundleOpts::new(6, "2018.4.36f1");
        f(&mut o);
        ("2018.4.36f1", 17, Layout::U2018_4, o)
    };
    let new = |f: fn(&mut BundleOpts)| {
        let mut o = BundleOpts::new(8, "2022.3.62f1");
        f(&mut o);
        ("2022.3.62f1", 22, Layout::U2022_3, o)
    };
    let cases = [
        old(|_| {}),
        old(|o| o.blocks = vec![0]),
        old(|o| o.blocks = vec![3, 3]),
        old(|o| o.blocks = vec![1]),
        old(|o| o.info_at_end = true),
        old(|o| o.info = 1),
        old(|o| o.info = 2),
        {
            let mut o = BundleOpts::new(6, "5.6.6f2");
            o.blocks = vec![2, 1, 0];
            ("5.6.6f2", 17, Layout::U5_6, o)
        },
        {
            let mut o = BundleOpts::new(7, "2019.4.40f1");
            o.info_at_end = true;
            ("2019.4.40f1", 21, Layout::U2019_4, o)
        },
        new(|_| {}),
        new(|o| o.padding_flag = true),
        new(|o| {
            o.padding_flag = true;
            o.info_at_end = true;
            o.info = 2;
        }),
    ];
    for (unity, version, layout, opts) in cases {
        let file = texture_file(unity, version, layout, &px);
        let bytes = bundle(
            &opts,
            &[("CAB-test", &file, 4), ("CAB-test.resS", &ress, 0)],
        );
        assert!(is_bundle(&bytes));
        let parsed =
            Bundle::parse(&bytes).unwrap_or_else(|e| panic!("{unity} {:?}: {e}", opts.blocks));
        assert_eq!(parsed.serialized_files().count(), 1, "{unity}");
        let dir = TempDir::new("bundle");
        let assets = Assets::open(dir.file("b", &bytes)).unwrap();
        assert_eq!(
            assets.decode_texture(7).unwrap().rgba,
            want,
            "{unity} {:?}",
            opts.blocks
        );
    }
}

#[test]
fn encrypted_and_ambiguous_bundles_are_refused() {
    let file = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[0; 48]),
    );
    // Old engines used 0x200 for encryption.
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.extra_flags = 0x200;
    let b = bundle(&o, &[("CAB-a", &file, 4)]);
    assert!(matches!(Bundle::parse(&b), Err(Error::Encrypted)));
    // New engines use 0x1000.
    let mut o = BundleOpts::new(8, "2022.3.62f1");
    o.extra_flags = 0x1000;
    let b = bundle(&o, &[("CAB-a", &file, 4)]);
    assert!(matches!(Bundle::parse(&b), Err(Error::Encrypted)));
    // A stripped engine version cannot tell the two meanings of 0x200 apart.
    let mut o = BundleOpts::new(8, "0.0.0");
    o.padding_flag = true;
    let b = bundle(&o, &[("CAB-a", &file, 4)]);
    match Bundle::parse(&b) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("stripped"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn stripped_file_version_takes_the_bundles() {
    let file = texture_file("0.0.0", 22, Layout::U2022_3, &Pixels::Inline(&[1; 48]));
    let b = bundle(&BundleOpts::new(8, "2022.3.62f1"), &[("CAB-a", &file, 4)]);
    let assets = Assets::open(TempDir::new("strip").file("b", &b)).unwrap();
    assert_eq!(assets.file().unity_version(), "2022.3.62f1");
    assert!(assets.decode_texture(7).is_ok());
}

#[test]
fn bundles_with_several_serialized_files() {
    let a = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[1; 48]),
    );
    let b = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[2; 48]),
    );
    let bytes = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("CAB-a", &a, 4), ("empty", &[], 0), ("CAB-b", &b, 4)],
    );
    let dir = TempDir::new("multi");
    match Assets::open(dir.file("b", &bytes)) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("CAB-a, CAB-b"), "{msg}"),
        other => panic!("{other:?}"),
    }
    let bundle = std::sync::Arc::new(Bundle::parse(&bytes).unwrap());
    assert_eq!(bundle.entries()[1].size(), 0);
    for (path, byte) in [("CAB-a", 1u8), ("archive:/x/CAB-b", 2)] {
        let assets = Assets::from_bundle(bundle.clone(), path).unwrap();
        assert!(assets
            .decode_texture(7)
            .unwrap()
            .rgba
            .chunks(4)
            .all(|p| p[..3] == [byte; 3]));
    }
    assert!(matches!(
        Assets::from_bundle(bundle, "CAB-z"),
        Err(Error::NotFound(_))
    ));
    let none = bundle_with_no_serialized_file();
    match Assets::open(dir.file("n", &none)) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("no serialized file"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

fn bundle_with_no_serialized_file() -> Vec<u8> {
    bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("data.resS", &[1, 2, 3], 0)],
    )
}

#[test]
fn not_unity_files_say_so() {
    let dir = TempDir::new("notunity");
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0";
    assert!(matches!(
        Assets::open(dir.file("x.png", png)),
        Err(Error::NotUnity(_))
    ));
    for sig in ["UnityWeb", "UnityRaw"] {
        let mut b = sig.as_bytes().to_vec();
        b.extend([0; 40]);
        match Assets::open(dir.file("w", &b)) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains(sig), "{msg}"),
            other => panic!("{other:?}"),
        }
    }
    match SerializedFile::parse(bundle_with_no_serialized_file()) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("asset bundle"), "{msg}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn empty_textures_are_listed_and_say_so() {
    let empty = texture(
        Layout::U5_6,
        false,
        "Font Texture",
        0,
        0,
        format::ALPHA8,
        &Pixels::Inline(&[]),
        &[],
    );
    let file = serialized(17, "5.6.6f2", false, 19, &[(7, TEXTURE_2D, empty)]);
    let assets = Assets::open(TempDir::new("empty").file("t.assets", &file)).unwrap();
    assert_eq!(assets.texture(7).unwrap().width, 0);
    assert!(matches!(
        assets.decode_texture(7),
        Err(Error::EmptyTexture(_))
    ));
}

#[test]
fn switch_swizzled_textures_are_refused() {
    let mut blob = vec![0u8; 12];
    blob[8] = 4; // 16 GOBs per block
    let obj = texture(
        Layout::U2022_3,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &blob,
    );
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        38,
        &[(7, TEXTURE_2D, obj.clone())],
    );
    let assets = Assets::open(TempDir::new("switch").file("t.assets", &file)).unwrap();
    match assets.decode_texture(7) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("Switch"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // The same blob on another platform means nothing.
    let file = serialized(22, "2022.3.62f1", false, 19, &[(7, TEXTURE_2D, obj)]);
    let assets = Assets::open(TempDir::new("switch").file("t.assets", &file)).unwrap();
    assert!(assets.decode_texture(7).is_ok());
}

#[test]
fn unsupported_formats_name_the_texture() {
    let obj = texture(
        Layout::U2018_4,
        false,
        "bc7",
        4,
        4,
        25,
        &Pixels::Inline(&[0; 16]),
        &[],
    );
    let file = serialized(17, "2018.4.36f1", false, 19, &[(7, TEXTURE_2D, obj)]);
    let assets = Assets::open(TempDir::new("fmt").file("t.assets", &file)).unwrap();
    match assets.decode_texture(7) {
        Err(Error::UnsupportedTextureFormat {
            texture,
            format: 25,
        }) => assert_eq!(texture, "bc7"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_truncation_is_an_error() {
    let pixels = rgba_4x4();
    let file = serialized(
        17,
        "5.6.6f2",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U5_6, &pixels))],
    );
    let bytes = bundle(&BundleOpts::new(6, "5.6.6f2"), &[("CAB-t", &file, 4)]);
    for n in 0..bytes.len() {
        assert!(
            Bundle::parse(&bytes[..n]).is_err(),
            "bundle truncated to {n} parsed"
        );
    }
    for n in 0..file.len() {
        assert!(
            SerializedFile::parse(file[..n].to_vec()).is_err(),
            "file truncated to {n} parsed"
        );
    }
}
