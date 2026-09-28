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

fn open_file(tag: &str, file: &[u8]) -> (TempDir, Result<Assets, Error>) {
    let dir = TempDir::new(tag);
    let path = dir.file("t.assets", file);
    let assets = Assets::open(&path);
    (dir, assets)
}

#[test]
fn inline_textures_in_every_layout() {
    let pixels = rgba_4x4();
    let cases = [
        (17, "5.6.6f2", Layout::U5_6),
        (17, "2017.1.0f1", Layout::U2017_1),
        (17, "2018.4.36f1", Layout::U2018_4),
        (21, "2019.3.0f6", Layout::U2018_4),
        (21, "2019.3.5f1", Layout::U2019_3),
        (21, "2019.4.8f1", Layout::U2019_3),
        (21, "2019.4.40f1", Layout::U2019_4),
        (22, "2020.1.17f1", Layout::U2020_1),
        (22, "2020.3.48f1", Layout::U2021_3),
        (22, "2021.3.45f1", Layout::U2021_3),
        (22, "2022.1.24f1", Layout::U2021_3),
        (22, "2022.3.62f1", Layout::U2022_3),
        (22, "2023.2.20f1", Layout::U6000),
        (22, "6000.0.23f1", Layout::U6000),
        (22, "6000.4.1b3", Layout::U6000),
    ];
    for (version, unity, layout) in cases {
        let file = serialized(
            version,
            unity,
            false,
            19,
            &[(7, TEXTURE_2D, inline(layout, &pixels))],
        );
        let (_d, assets) = open_file("inline", &file);
        let assets = assets.unwrap();
        let listed = assets.textures(|_| true);
        assert_eq!(listed.len(), 1, "{unity}");
        assert_eq!(
            (listed[0].path_id, listed[0].name.as_deref()),
            (7, Some("t"))
        );
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
fn engines_outside_the_known_range_are_refused() {
    for unity in ["6000.5.0f1", "2023.2.0a17", "5.4.6f3", "0.0.0"] {
        let file = serialized(
            22,
            unity,
            false,
            19,
            &[(7, TEXTURE_2D, inline(Layout::U6000, &rgba_4x4()))],
        );
        let (_d, assets) = open_file("range", &file);
        match assets.unwrap().texture(7) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains(unity), "{msg}"),
            other => panic!("{unity}: {other:?}"),
        }
    }
}

#[test]
fn a_wrong_layout_is_caught_not_misread() {
    // A 2018.4 object labelled 5.6.
    let file = serialized(
        17,
        "5.6.6f2",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U2018_4, &rgba_4x4()))],
    );
    let (_d, assets) = open_file("wrong", &file);
    let result = assets.unwrap().decode_texture(7);
    assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
}

#[test]
fn metadata_sections_are_stepped_over() {
    // Type trees (both node sizes), a MonoBehaviour type with its script ID, scripts and
    // externals, each before the objects or after them.
    for version in [17, 19, 21, 22] {
        let extras = Extras {
            type_trees: true,
            mono: true,
            scripts: 3,
            externals: vec!["sharedassets1.assets".into(), "archive:/CAB-x/CAB-x".into()],
        };
        let objects = [(7, TEXTURE_2D, inline(Layout::U2018_4, &rgba_4x4()))];
        let file = serialized_with(version, "2018.4.36f1", false, 19, &objects, &extras);
        let parsed =
            SerializedFile::parse(file.clone()).unwrap_or_else(|e| panic!("v{version}: {e}"));
        assert!(parsed.has_type_trees());
        assert_eq!(parsed.types().len(), 2);
        assert_eq!(
            parsed.externals()[1].path,
            "archive:/CAB-x/CAB-x",
            "v{version}"
        );
        let (_d, assets) = open_file("meta", &file);
        assert_eq!(
            assets.unwrap().decode_texture(7).unwrap().rgba,
            flip(&rgba_4x4(), 4)
        );
    }
}

#[test]
fn future_format_versions_are_unsupported_not_foreign() {
    let mut file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U2022_3, &rgba_4x4()))],
    );
    file[8..12].copy_from_slice(&23u32.to_be_bytes());
    match SerializedFile::parse(file) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("23"), "{msg}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn file_sizes_must_agree() {
    let mut file = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U2018_4, &rgba_4x4()))],
    );
    file.push(0);
    assert!(matches!(
        SerializedFile::parse(file),
        Err(Error::Invalid(_))
    ));
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
    let (_d, assets) = open_file("be", &file);
    let assets = assets.unwrap();
    assert!(assets.file().big_endian());
    assert_eq!(assets.decode_texture(7).unwrap().rgba, flip(&pixels, 4));
}

#[test]
fn several_objects() {
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
        ],
    );
    let (_d, assets) = open_file("multi", &file);
    let assets = assets.unwrap();
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
    let (_d, assets) = open_file("mip", &file);
    assert_eq!(
        assets.unwrap().decode_texture(7).unwrap().rgba,
        flip(&rgba_4x4(), 4)
    );
}

#[test]
fn texture_data_is_the_first_mip_and_refuses_unknown_formats() {
    let dir = TempDir::new("data");
    let mut pixels = rgba_4x4();
    pixels.extend([9u8; 16]);
    let gone = Pixels::Streamed {
        path: "gone.resS",
        offset: 0,
        size: 16,
    };
    let file = serialized(
        17,
        "2018.4.36f1",
        false,
        19,
        &[
            (7, TEXTURE_2D, inline(Layout::U2018_4, &pixels)),
            (
                8,
                TEXTURE_2D,
                texture(Layout::U2018_4, false, "bc7", 4, 4, 25, &gone, &[]),
            ),
        ],
    );
    let assets = Assets::open(dir.file("t.assets", &file)).unwrap();
    assert_eq!(assets.texture(7).unwrap().data(&dir.0).unwrap().len(), 64);
    match assets.texture(8).unwrap().data(&dir.0) {
        Err(Error::UnsupportedTextureFormat { format: 25, .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn streamed_textures_beside_the_file() {
    let dir = TempDir::new("stream");
    let pixels = rgba_4x4();
    let mut ress = vec![1, 2, 3];
    ress.extend(&pixels);
    dir.file("t.assets.resS", &ress);
    for layout in [Layout::U2018_4, Layout::U2021_3] {
        let px = Pixels::Streamed {
            path: "t.assets.resS",
            offset: 3,
            size: 64,
        };
        let obj = texture(layout, false, "t", 4, 4, format::RGBA32, &px, &[]);
        let unity = if matches!(layout, Layout::U2018_4) {
            "2018.4.36f1"
        } else {
            "2021.3.45f1"
        };
        let path = dir.file(
            "t.assets",
            &serialized(22, unity, false, 19, &[(7, TEXTURE_2D, obj)]),
        );
        assert_eq!(
            Assets::open(&path).unwrap().decode_texture(7).unwrap().rgba,
            flip(&pixels, 4)
        );
    }
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
        {
            // The first release to read 0x200 as padding.
            let mut o = BundleOpts::new(7, "2020.3.34f1");
            o.padding_flag = true;
            ("2020.3.34f1", 22, Layout::U2021_3, o)
        },
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
fn bytes_after_a_bundle_are_ignored() {
    let file = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[3; 48]),
    );
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.info_at_end = true;
    let mut b = bundle(&o, &[("CAB-a", &file, 4)]);
    let len = b.len() as i64;
    let at = b.windows(6).position(|w| w == b"36f1\0\0").unwrap() + 5;
    b[at..at + 8].copy_from_slice(&len.to_be_bytes());
    b.extend([0xee; 100]);
    assert!(Bundle::parse(&b).is_ok());
}

#[test]
fn encrypted_and_ambiguous_bundles_are_refused() {
    let file = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[0; 48]),
    );
    let refused = |format: u32, revision: &str, extra: u32| {
        let mut o = BundleOpts::new(format, revision);
        o.extra_flags = extra;
        Bundle::parse(&bundle(&o, &[("CAB-a", &file, 4)]))
    };
    // Old engines used 0x200 for encryption, through 2020.3.33.
    assert!(matches!(
        refused(6, "2018.4.36f1", 0x200),
        Err(Error::Encrypted)
    ));
    assert!(matches!(
        refused(7, "2020.3.33f1", 0x200),
        Err(Error::Encrypted)
    ));
    // New engines use 0x400 and 0x1000.
    assert!(matches!(
        refused(8, "2022.3.62f1", 0x1000),
        Err(Error::Encrypted)
    ));
    assert!(matches!(
        refused(8, "2022.3.62f1", 0x400),
        Err(Error::Encrypted)
    ));
    // With the version stripped, the new bits are still encryption, and 0x200 is ambiguous.
    assert!(matches!(refused(8, "0.0.0", 0x1000), Err(Error::Encrypted)));
    match refused(8, "0.0.0", 0x200) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("stripped"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn bundle_tables_must_agree_with_the_data() {
    let file = texture_file(
        "2018.4.36f1",
        17,
        Layout::U2018_4,
        &Pixels::Inline(&[0; 48]),
    );
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![0];
    let good = bundle(&o, &[("CAB-a", &file, 4)]);
    assert!(Bundle::parse(&good).is_ok());
    // An entry whose size runs past the data.
    let size = file.len() as i64;
    let at = good
        .windows(8)
        .position(|w| w == size.to_be_bytes())
        .unwrap();
    let mut past = good.clone();
    past[at..at + 8].copy_from_slice(&(size + 1).to_be_bytes());
    match Bundle::parse(&past) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("runs past"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // A stored block whose compressed and uncompressed sizes disagree.
    let sizes = [(size as u32).to_be_bytes(), (size as u32).to_be_bytes()].concat();
    let at = good.windows(8).position(|w| w == sizes).unwrap();
    let mut short = good.clone();
    short[at + 4..at + 8].copy_from_slice(&(size as u32 - 1).to_be_bytes());
    match Bundle::parse(&short) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("stored block"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // And one claiming more stored bytes than it produces (the directory follows the block).
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![0];
    o.info_at_end = true;
    let good = bundle(&o, &[("CAB-a", &file, 4)]);
    let at = good.windows(8).position(|w| w == sizes).unwrap();
    let mut long = good.clone();
    long[at + 4..at + 8].copy_from_slice(&(size as u32 + 1).to_be_bytes());
    match Bundle::parse(&long) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("stored block"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn stripped_file_version_takes_the_bundles() {
    let file = texture_file("0.0.0", 22, Layout::U2022_3, &Pixels::Inline(&[1; 48]));
    let b = bundle(&BundleOpts::new(8, "2022.3.62f1"), &[("CAB-a", &file, 4)]);
    let dir = TempDir::new("strip");
    let assets = Assets::open(dir.file("b", &b)).unwrap();
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
        Err(Error::Unsupported(msg)) => assert!(msg.contains(r#""CAB-a", "CAB-b""#), "{msg}"),
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
    let (_d, assets) = open_file("empty", &file);
    let assets = assets.unwrap();
    assert_eq!(assets.texture(7).unwrap().width, 0);
    assert!(matches!(
        assets.decode_texture(7),
        Err(Error::EmptyTexture(_))
    ));
}

#[test]
fn console_textures_are_refused() {
    let obj = texture(
        Layout::U2022_3,
        false,
        "t",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    for platform in [31, 38, 44] {
        let file = serialized(
            22,
            "2022.3.62f1",
            false,
            platform,
            &[(7, TEXTURE_2D, obj.clone())],
        );
        let (_d, assets) = open_file("console", &file);
        match assets.unwrap().decode_texture(7) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains("console"), "{msg}"),
            other => panic!("{platform}: {other:?}"),
        }
    }
    let file = serialized(22, "2022.3.62f1", false, 19, &[(7, TEXTURE_2D, obj)]);
    let (_d, assets) = open_file("console", &file);
    assert!(assets.unwrap().decode_texture(7).is_ok());
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
    let (_d, assets) = open_file("fmt", &file);
    match assets.unwrap().decode_texture(7) {
        Err(Error::UnsupportedTextureFormat {
            texture,
            format: 25,
            ..
        }) => {
            assert_eq!(texture.as_deref(), Some("bc7"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn asking_for_the_wrong_class_says_so() {
    let file = serialized(17, "2018.4.36f1", false, 19, &[(7, SPRITE, vec![0; 16])]);
    let (_d, assets) = open_file("class", &file);
    assert!(matches!(
        assets.unwrap().texture(7),
        Err(Error::WrongClass {
            path_id: 7,
            found: 213,
            expected: 28,
            ..
        })
    ));
}

#[test]
fn every_truncation_is_an_error() {
    let file = serialized(
        17,
        "5.6.6f2",
        false,
        19,
        &[(7, TEXTURE_2D, inline(Layout::U5_6, &rgba_4x4()))],
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
