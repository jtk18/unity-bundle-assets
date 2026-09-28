//! One test per defect the red-team rounds found, so none can come back unnoticed.

mod common;
use common::*;
use std::sync::Arc;
use unity_bundle_assets::{
    decode, Assets, Bundle, Error, Image, LimitKind, Limits, SerializedFile, Sprite, Texture2D,
};

const TEX: i64 = 10;
const RECT: u32 = 0b10;
const PACKED: u32 = 0b01;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn rgba_texture(layout: Layout, name: &str, w: i32, h: i32, px: &Pixels) -> Vec<u8> {
    texture(layout, false, name, w, h, format::RGBA32, px, &[])
}

fn file_2018(objects: &[(i64, i32, Vec<u8>)]) -> Vec<u8> {
    serialized(17, "2018.4.36f1", false, 19, objects)
}

// Expected pixels from Pillow's BCn decoder (what UnityPy uses), rows flipped to top first.
const DXT1_IN: &str = "8dbc277da03d24278ba7bfbeb9369f2432b75e6f958203443f72ea9906482390";
const DXT1_OUT: &str = "b5e794ff6bebf7ffb5e794ff6bebf7ff7345ffff7345ffff9c3c52ff8740a8ff83e9d6ffb5e794ffb5e794ffb5e794ff000000007345ffff8740a8ff7345ffff9ce8b5ffb5e794ffb5e794ff9ce8b5ff7345ffff8740a8ff7345ffff9c3c52ff6bebf7ff6bebf7ff6bebf7ff9ce8b5ff8740a8ff9c3c52ff7345ffff7345ffff919f49ff7ba639ffa7985affbd926bffa5f35affbdd7ffffb1e5acffa5f35affbd926bff7ba639ffa7985affbd926bff0000000000000000bdd7ffffb1e5acff7ba639ff919f49ff919f49ffbd926bffb1e5acffbdd7ffff00000000a5f35affbd926bffbd926bffa7985affa7985affbdd7ffffb1e5acff00000000b1e5acff";
const DXT5_IN: &str = "db683c1c12cf1341ce5c85ac84a20e5d46fc366e10165123a461476118e72be49e11ec390b6679f65752de71f58b6e5c4db3aae6c7a5e82530541f5cfc063ad8";
const DXT5_OUT: &str = "5249bd25683de3617338f74d7338f725528684005484ad755a82ffb35783d6b35d43d039683de3615d43d04d7338f7615484ad9e5484ad8a5783d6615286848a683de3755d43d0395249bd895d43d09e5484ad005a82ffff528684b3528684007338f7617338f74d683de325683de361528684615783d69e5783d6615783d675ad922968919441caad9229dbad9229ca633421d76328390063302946632c31fc75975a78919441685a9a73785a9a7368632c31006330296a633029b36334214675975a685a9a73a975975aa975975adb632c310063283946633029b3632c31465a9a73a9ad9229785a9a73db75975a88633421006330290063283946633421ff";

#[test]
fn dxt_decoding_matches_the_reference_decoder() {
    let dxt1 = decode::decode(format::DXT1, 8, 8, &hex(DXT1_IN)).unwrap();
    assert_eq!(dxt1, hex(DXT1_OUT), "DXT1");
    let dxt5 = decode::decode(format::DXT5, 8, 8, &hex(DXT5_IN)).unwrap();
    assert_eq!(dxt5, hex(DXT5_OUT), "DXT5");
}

#[test]
fn format_numbers_are_unitys() {
    use unity_bundle_assets::decode::format as crate_format;
    assert_eq!(
        [
            crate_format::ALPHA8,
            crate_format::RGB24,
            crate_format::RGBA32,
            crate_format::ARGB32,
            crate_format::DXT1,
            crate_format::DXT5,
            crate_format::BGRA32
        ],
        [1, 3, 4, 5, 10, 12, 14]
    );
}

#[test]
fn texture_streams_may_not_overlap() {
    let dir = TempDir::new("overlap");
    dir.file("t.assets.resS", &[7; 128]);
    let at = |offset: u64| Pixels::Streamed {
        path: "t.assets.resS",
        offset,
        size: 64,
    };
    let file = file_2018(&[
        (
            1,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "a", 4, 4, &at(0)),
        ),
        (
            2,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "b", 4, 4, &at(0)),
        ),
        (
            3,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "c", 4, 4, &at(32)),
        ),
        (
            4,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "d", 4, 4, &at(64)),
        ),
    ]);
    let a = Assets::open(dir.file("t.assets", &file)).unwrap();
    assert!(a.decode_texture(1).is_ok());
    assert!(
        a.decode_texture(1).is_ok(),
        "the same texture again is fine"
    );
    for aliased in [2, 3] {
        match a.decode_texture(aliased) {
            Err(Error::Invalid(msg)) => assert!(msg.contains("already read"), "{msg}"),
            other => panic!("{aliased}: {other:?}"),
        }
    }
    assert!(a.decode_texture(4).is_ok(), "an adjacent range is fine");
}

fn two_file_bundle(same_range: bool, limits: Limits) -> Arc<Bundle> {
    let stream = |name: &'static str| Pixels::Streamed {
        path: name,
        offset: if same_range || name.ends_with("a.resS") {
            0
        } else {
            64
        },
        size: 64,
    };
    let a = file_2018(&[(
        1,
        TEXTURE_2D,
        rgba_texture(
            Layout::U2018_4,
            "a",
            4,
            4,
            &stream("archive:/CAB-a/CAB-a.resS"),
        ),
    )]);
    let b = file_2018(&[(
        1,
        TEXTURE_2D,
        rgba_texture(
            Layout::U2018_4,
            "b",
            4,
            4,
            &stream("archive:/CAB-a/CAB-a.resS"),
        ),
    )]);
    let bytes = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[
            ("CAB-a", &a, 4),
            ("CAB-b", &b, 4),
            ("CAB-a.resS", &[5; 128], 0),
        ],
    );
    Arc::new(Bundle::parse_with(&bytes, limits).unwrap())
}

#[test]
fn files_in_one_bundle_share_its_stream_ranges_and_budget() {
    // Two serialized files reading the same range: the second is refused.
    let b = two_file_bundle(true, Limits::default());
    let first = Assets::from_bundle(b.clone(), "CAB-a").unwrap();
    let second = Assets::from_bundle(b, "CAB-b").unwrap();
    assert!(first.decode_texture(1).is_ok());
    assert!(matches!(second.decode_texture(1), Err(Error::Invalid(_))));

    // Different ranges, but one budget: 16 pixels each, 20 allowed.
    let b = two_file_bundle(false, Limits::DEFAULT.with_max_total_work(20));
    let first = Assets::from_bundle(b.clone(), "CAB-a").unwrap();
    let second = Assets::from_bundle(b, "CAB-b").unwrap();
    assert!(first.decode_texture(1).is_ok());
    assert!(matches!(
        second.decode_texture(1),
        Err(Error::LimitExceeded {
            kind: LimitKind::TotalWork,
            ..
        })
    ));
    assert_eq!(first.work_done(), 16);
}

#[test]
fn bundle_entries_may_not_overlap() {
    let opts = BundleOpts::new(6, "2018.4.36f1");
    let good = bundle(&opts, &[("a.resS", &[1; 40], 0), ("b.resS", &[2; 40], 0)]);
    assert!(Bundle::parse(&good).is_ok());
    // The second entry's offset (40) sits before its size (40) and flags; point it at 0.
    let entry = [40i64.to_be_bytes(), 40i64.to_be_bytes()].concat();
    let at = good.windows(16).position(|w| w == entry).unwrap();
    let mut bad = good;
    bad[at..at + 8].copy_from_slice(&0i64.to_be_bytes());
    match Bundle::parse(&bad) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("overlap"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn engine_version_strings_are_checked() {
    let long = format!("2022.3.1f1{}", "\u{1}".repeat(100));
    let file = serialized(22, &long, false, 19, &[]);
    assert!(matches!(
        SerializedFile::parse(file),
        Err(Error::NotUnity(_))
    ));
    let b = bundle(&BundleOpts::new(6, &"2".repeat(40)), &[("a.resS", &[1], 0)]);
    assert!(matches!(Bundle::parse(&b), Err(Error::NotUnity(_))));
}

#[test]
fn object_count_is_limited() {
    let tex = || rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = file_2018(&[(1, TEXTURE_2D, tex()), (2, TEXTURE_2D, tex())]);
    let limits = Limits::DEFAULT.with_max_objects(1);
    assert!(matches!(
        SerializedFile::parse_with(file.clone(), limits),
        Err(Error::LimitExceeded {
            kind: LimitKind::Objects,
            ..
        })
    ));
    assert!(SerializedFile::parse_with(file, Limits::DEFAULT.with_max_objects(2)).is_ok());
}

#[test]
fn refused_and_failed_work_is_not_charged() {
    let dir = TempDir::new("charge");
    let file = file_2018(&[
        (
            1,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "big", 8, 8, &Pixels::Inline(&[1; 256])),
        ),
        (
            2,
            TEXTURE_2D,
            rgba_texture(Layout::U2018_4, "ok", 4, 4, &Pixels::Inline(&rgba_4x4())),
        ),
        (
            3,
            TEXTURE_2D,
            rgba_texture(
                Layout::U2018_4,
                "gone",
                4,
                4,
                &Pixels::Streamed {
                    path: "gone.resS",
                    offset: 0,
                    size: 64,
                },
            ),
        ),
    ]);
    let path = dir.file("t.assets", &file);
    let a = Assets::open_with(&path, Limits::DEFAULT.with_max_total_work(20)).unwrap();
    assert!(matches!(
        a.decode_texture(1),
        Err(Error::LimitExceeded {
            kind: LimitKind::TotalWork,
            ..
        })
    ));
    for _ in 0..3 {
        assert!(matches!(a.decode_texture(3), Err(Error::Io { .. })));
    }
    assert_eq!(a.work_done(), 0);
    assert!(a.decode_texture(2).is_ok());
    assert_eq!(a.work_done(), 16);
}

const TRIANGLE: Mesh = Mesh {
    vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
    indices: &[0, 1, 2],
    ..Mesh::BASE
};

fn tight(mesh: &Mesh, settings: u32) -> Vec<u8> {
    let r = [0.0, 0.0, 4.0, 4.0];
    sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        1,
        0,
        TEX,
        0,
        r,
        settings,
        1.0,
        mesh,
    )
}

fn one_sprite(sprite_bytes: Vec<u8>, limits: Limits) -> (TempDir, Assets, Sprite) {
    let dir = TempDir::new("sprite");
    let tex = rgba_texture(Layout::U2022_3, "tex", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, sprite_bytes)],
    );
    let a = Assets::open_with(dir.file("s.assets", &file), limits).unwrap();
    let mut list = a.sprites(|_| true);
    assert!(list.skipped.is_empty(), "{:?}", list.skipped);
    let s = list.sprites.remove(0);
    (dir, a, s)
}

fn kept(img: &Image) -> Vec<u8> {
    img.rgba
        .chunks(4)
        .filter(|p| p[3] != 0)
        .map(|p| p[0] / 4)
        .collect()
}

#[test]
fn mask_and_copy_work_are_charged() {
    // Decoding 16 pixels, masking a 4x4 box, copying 16: 48 in all.
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::DEFAULT.with_max_total_work(48));
    assert!(a.export(&s).is_ok());
    assert_eq!(a.work_done(), 48);
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::DEFAULT.with_max_total_work(47));
    assert!(matches!(
        a.export(&s),
        Err(Error::LimitExceeded {
            kind: LimitKind::TotalWork,
            ..
        })
    ));
}

#[test]
fn tight_sprites_need_a_mesh_and_packing_a_known_rotation() {
    let (_d, mut a, s) = one_sprite(tight(&Mesh::BASE, 0), Limits::default());
    match a.export(&s) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("empty"), "{msg}"),
        other => panic!("{other:?}"),
    }
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, PACKED | RECT | 5 << 2), Limits::default());
    match a.export(&s) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("rotation 5"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_rotated_tight_sprite_is_masked_in_sprite_space() {
    // Crop 3x2, turned a quarter to 2x3; the mesh covers the lower left of the 2x3 sprite.
    let mesh = Mesh {
        vertices: &[[0.0, 0.0], [2.0, 0.0], [0.0, 3.0]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let r = [0.0, 0.0, 2.0, 3.0];
    let crop = [0.0, 0.0, 3.0, 2.0];
    let s = sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        1,
        0,
        TEX,
        0,
        crop,
        PACKED | 4 << 2,
        1.0,
        &mesh,
    );
    let (_d, mut a, s) = one_sprite(s, Limits::default());
    let img = a.export(&s).unwrap();
    assert_eq!((img.width, img.height), (2, 3));
    assert_eq!(kept(&img), [0, 1, 2, 6]);
}

#[test]
fn the_mesh_maps_through_pivot_scale_and_offset() {
    // Full rect 5x4 with the 4x4 texture rect one pixel in; pivot (0.4, 0.75), 100 pixels per
    // unit: pixel = vertex * 100 + (5 * 0.4 - 1, 4 * 0.75) = vertex * 100 + (1, 3).
    let mesh = Mesh {
        vertices: &[[-0.01, -0.03], [0.03, -0.03], [-0.01, 0.01]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let s = sprite_full(
        false,
        false,
        "s",
        [0.0, 0.0, 5.0, 4.0],
        [0.4, 0.75],
        1,
        0,
        TEX,
        0,
        [0.0, 0.0, 4.0, 4.0],
        0,
        1.0,
        &mesh,
        100.0,
        [1.0, 0.0],
    );
    let (_d, mut a, s) = one_sprite(s, Limits::default());
    assert_eq!(
        kept(&a.export(&s).unwrap()),
        [12, 8, 9, 4, 5, 6, 0, 1, 2, 3]
    );
}

#[test]
fn export_decodes_the_right_texture_after_switching() {
    let dir = TempDir::new("cache");
    let other: Vec<u8> = (100..164).collect();
    let r = [0.0, 0.0, 1.0, 1.0];
    let on = |tex: i64| {
        sprite(
            false,
            false,
            "s",
            r,
            [0.0, 0.0],
            1,
            0,
            tex,
            0,
            r,
            RECT,
            1.0,
            &Mesh::BASE,
        )
    };
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (
                TEX,
                TEXTURE_2D,
                rgba_texture(Layout::U2022_3, "a", 4, 4, &Pixels::Inline(&rgba_4x4())),
            ),
            (
                11,
                TEXTURE_2D,
                rgba_texture(Layout::U2022_3, "b", 4, 4, &Pixels::Inline(&other)),
            ),
            (1, SPRITE, on(TEX)),
            (2, SPRITE, on(11)),
        ],
    );
    let mut a = Assets::open(dir.file("s.assets", &file)).unwrap();
    let list = a.sprites(|_| true);
    let by_id = |id: i64| list.sprites.iter().find(|s| s.path_id == id).unwrap();
    for (id, want) in [(1, 0u8), (2, 100), (1, 0), (2, 100)] {
        assert_eq!(a.export(by_id(id)).unwrap().rgba[0], want, "sprite {id}");
    }
}

#[test]
fn cut_refuses_rather_than_panics_on_huge_images() {
    let width = 16_777_219u32;
    let wide = Image::new(width, 1, vec![0; width as usize * 4]).unwrap();
    let r = [0.0, 0.0, 16_777_220.0, 1.0];
    let s = sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        1,
        0,
        TEX,
        0,
        r,
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let (_d, a, s) = one_sprite(s, Limits::default());
    assert!(matches!(a.cut(&s, &wide), Err(Error::Invalid(_))));
}

#[test]
fn stream_names_that_are_devices_or_data_streams_are_refused() {
    let dir = TempDir::new("names");
    for name in [
        "CON.resS",
        "nul.resource",
        "com1.resS",
        "a:b.resS",
        "LPT9.resS",
    ] {
        let px = Pixels::Streamed {
            path: name,
            offset: 0,
            size: 16,
        };
        let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::ALPHA8, &px, &[]);
        let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
        assert!(
            matches!(a.decode_texture(7), Err(Error::Unsupported(_))),
            "{name}"
        );
    }
    // Names that only look like devices are ordinary files.
    for name in ["COM0.resS", "COM10.resS", "CONSOLE.resS", "lpt.resS"] {
        dir.file(name, &[3; 16]);
        let px = Pixels::Streamed {
            path: name,
            offset: 0,
            size: 16,
        };
        let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::ALPHA8, &px, &[]);
        let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
        assert!(a.decode_texture(7).is_ok(), "{name}");
    }
}

#[test]
fn an_atlas_must_end_where_its_fields_do() {
    let entries = [(5, TEX, [0.0, 0.0, 2.0, 2.0], RECT, 1.0)];
    let read = |extra: &[u8]| {
        let mut obj = atlas(false, &entries);
        obj.extend_from_slice(extra);
        let file = SerializedFile::parse(serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(30, SPRITE_ATLAS, obj)],
        ))
        .unwrap();
        unity_bundle_assets::SpriteAtlas::read(&file, &file.objects()[0]).map(|_| ())
    };
    assert!(read(&[]).is_ok());
    assert!(matches!(read(&[0; 4]), Err(Error::Invalid(_))));
}

#[cfg(unix)]
#[test]
fn hard_linked_stream_files_are_refused() {
    let outer = TempDir::new("hl-outer");
    outer.file("secret", &[0xaa; 16]);
    let dir = TempDir::new("hl-inner");
    std::fs::hard_link(outer.0.join("secret"), dir.0.join("x.resS")).unwrap();
    let px = Pixels::Streamed {
        path: "x.resS",
        offset: 0,
        size: 16,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::ALPHA8, &px, &[]);
    let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
    match a.decode_texture(7) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("hard links"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

/// A 2022.3 texture with its m_MipsStripped and m_MipCount replaced.
fn with_mips(stripped: i32, count: i32) -> Vec<u8> {
    let obj = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let pattern = [4i32, 4, 0, 0, format::RGBA32, 1]
        .map(i32::to_le_bytes)
        .concat();
    let at = obj.windows(24).position(|w| w == pattern).unwrap();
    let mut obj = obj;
    obj[at + 12..at + 16].copy_from_slice(&stripped.to_le_bytes());
    obj[at + 20..at + 24].copy_from_slice(&count.to_le_bytes());
    serialized(22, "2022.3.62f1", false, 19, &[(7, TEXTURE_2D, obj)])
}

#[test]
fn stripped_or_negative_mips_are_refused() {
    let dir = TempDir::new("mips");
    let a = Assets::open(dir.file("t.assets", &with_mips(0, 1))).unwrap();
    assert!(a.decode_texture(7).is_ok());
    let a = Assets::open(dir.file("t.assets", &with_mips(2, 1))).unwrap();
    assert!(matches!(a.texture(7), Err(Error::Unsupported(_))));
    let a = Assets::open(dir.file("t.assets", &with_mips(0, -7))).unwrap();
    assert!(matches!(a.texture(7), Err(Error::Invalid(_))));
}

#[test]
fn every_texture_layout_boundary() {
    let pixels = rgba_4x4();
    for (unity, layout) in [
        ("5.5.0f3", Layout::U5_6),
        ("2017.2.5f1", Layout::U2017_1),
        ("2017.3.0f3", Layout::U2017_3),
        ("2018.1.9f2", Layout::U2017_3),
        ("2018.2.0f2", Layout::U2018_4),
        ("2019.4.8f1", Layout::U2019_3),
        ("2019.4.9f1", Layout::U2019_4),
        ("2020.1.0f1", Layout::U2020_1),
        ("2020.2.0f1", Layout::U2021_3),
        ("2022.1.24f1", Layout::U2021_3),
        ("2022.2.0b2", Layout::U2021_3),
        ("2022.2.0b3", Layout::U2022_3),
        ("2023.1.20f1", Layout::U2022_3),
        ("2023.2.0f1", Layout::U6000),
    ] {
        let version = if unity.starts_with("2020") || unity > "2020" {
            22
        } else {
            17
        };
        let file = serialized(
            version,
            unity,
            false,
            19,
            &[(
                7,
                TEXTURE_2D,
                rgba_texture(layout, "t", 4, 4, &Pixels::Inline(&pixels)),
            )],
        );
        let dir = TempDir::new("gate");
        let a = Assets::open(dir.file("t.assets", &file)).unwrap();
        assert_eq!(
            a.decode_texture(7).unwrap().rgba,
            flip(&pixels, 4),
            "{unity}"
        );
    }
    // A beta before the boundary read with the later layout is caught, not misread.
    let file = serialized(
        22,
        "2022.2.0b2",
        false,
        19,
        &[(
            7,
            TEXTURE_2D,
            rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&pixels)),
        )],
    );
    let dir = TempDir::new("gate");
    assert!(Assets::open(dir.file("t.assets", &file))
        .unwrap()
        .texture(7)
        .is_err());
}

#[test]
fn dimensions_stop_at_unitys_maximum() {
    let dir = TempDir::new("dims");
    let row = |w: i32| {
        rgba_texture(
            Layout::U2018_4,
            "t",
            w,
            1,
            &Pixels::Inline(&vec![1; w as usize * 4]),
        )
    };
    let a = Assets::open(dir.file("a.assets", &file_2018(&[(7, TEXTURE_2D, row(16384))]))).unwrap();
    assert!(a.texture(7).is_ok());
    let a = Assets::open(dir.file("b.assets", &file_2018(&[(7, TEXTURE_2D, row(16385))]))).unwrap();
    assert!(matches!(a.texture(7), Err(Error::Invalid(_))));
    // No pixels and a zero width alone is not an empty texture: it is a misread.
    let obj = rgba_texture(Layout::U2018_4, "t", 0, 4, &Pixels::Inline(&[]));
    let a = Assets::open(dir.file("c.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
    assert!(matches!(a.texture(7), Err(Error::Invalid(_))));
}

#[test]
fn container_formats_outside_6_to_8_are_refused() {
    let good = bundle(&BundleOpts::new(6, "2018.4.36f1"), &[("a.resS", &[1], 0)]);
    for format in [5u32, 9, u32::MAX] {
        let mut b = good.clone();
        b[8..12].copy_from_slice(&format.to_be_bytes());
        assert!(
            matches!(Bundle::parse(&b), Err(Error::Unsupported(_))),
            "{format}"
        );
    }
}

#[test]
fn bundle_flag_boundaries() {
    let file = file_2018(&[(
        1,
        TEXTURE_2D,
        rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4())),
    )]);
    let parse = |format: u32, revision: &str, flags: u32, align: bool| {
        let mut o = BundleOpts::new(format, revision);
        o.extra_flags = flags;
        o.align_header = align;
        Bundle::parse(&bundle(&o, &[("CAB-a", &file, 4)]))
    };
    // A stripped version with no ambiguous flag is fine.
    assert!(parse(6, "0.0.0", 0, false).is_ok());
    // 0x200 means encryption before these releases and padding from them on.
    for (before, from) in [
        ("2021.3.1f1", "2021.3.2f1"),
        ("2022.1.0f1", "2022.1.1f1"),
        ("2020.3.33f1", "2020.3.34f1"),
    ] {
        assert!(
            matches!(parse(7, before, 0x200, false), Err(Error::Encrypted)),
            "{before}"
        );
        let mut o = BundleOpts::new(7, from);
        o.padding_flag = true;
        assert!(
            Bundle::parse(&bundle(&o, &[("CAB-a", &file, 4)])).is_ok(),
            "{from}"
        );
    }
    // From 2019.4.15 the header is padded even at format 6.
    assert!(parse(6, "2019.4.15f1", 0, true).is_ok());
    assert!(parse(6, "2019.4.14f1", 0, false).is_ok());
}

/// Set the 32-bit count at `at` to one more than `min_size`-byte items could fill.
fn overclaim(data: &mut [u8], at: usize, min_size: usize) {
    let left = data.len() - at - 4;
    let n = (left / min_size + 1) as u32;
    data[at..at + 4].copy_from_slice(&n.to_le_bytes());
}

#[test]
fn counts_are_checked_against_real_record_sizes() {
    let tex = rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = file_2018(&[(7, TEXTURE_2D, tex)]);
    // After the 20-byte header: the version string, its NUL, platform (4), type trees (1).
    let types_at = 20 + "2018.4.36f1".len() + 1 + 4 + 1;
    let mut f = file.clone();
    overclaim(&mut f, types_at, 23);
    assert!(matches!(
        SerializedFile::parse(f),
        Err(Error::BadLength { .. })
    ));
    let objects_at = types_at + 4 + 23;
    let mut f = file;
    overclaim(&mut f, objects_at, 20);
    assert!(matches!(
        SerializedFile::parse(f),
        Err(Error::BadLength { .. })
    ));

    // An atlas claiming one entry more than its bytes hold at 108 bytes each.
    let entries = [
        (5, TEX, [0.0, 0.0, 2.0, 2.0], RECT, 1.0),
        (6, TEX, [0.0, 0.0, 2.0, 2.0], RECT, 1.0),
    ];
    let mut atlas_obj = atlas(false, &entries);
    // Name "atlas" (4 + 5 + 3 padding), then the two empty lists.
    overclaim(&mut atlas_obj, 12 + 4 + 4, 108);
    let dir = TempDir::new("counts");
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(30, SPRITE_ATLAS, atlas_obj)],
    );
    let a = SerializedFile::open(dir.file("a.assets", &file)).unwrap();
    let err = unity_bundle_assets::SpriteAtlas::read(&a, &a.objects()[0]).unwrap_err();
    assert!(err.to_string().contains("bad length"), "{err}");
}

#[test]
fn debug_output_is_short() {
    let many = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
        indices: &[0, 1, 2].repeat(1000),
        ..Mesh::BASE
    };
    let (_d, a, s) = one_sprite(tight(&many, 0), Limits::default());
    assert!(format!("{s:?}").len() < 600, "{}", format!("{s:?}").len());
    assert!(format!("{:?}", a.sprites(|_| true)).len() < 100);
    assert!(format!("{:?}", a.texture(TEX).unwrap()).len() < 300);
    assert!(format!("{a:?}").len() < 600);
}

#[test]
fn object_readers_check_the_class() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let s = sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        1,
        0,
        TEX,
        0,
        r,
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = SerializedFile::parse(serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(1, SPRITE, s), (TEX, TEXTURE_2D, tex)],
    ))
    .unwrap();
    let sprite_obj = file.object(1).unwrap();
    let tex_obj = file.object(TEX).unwrap();
    assert!(matches!(
        Texture2D::read(&file, sprite_obj),
        Err(Error::WrongClass { found: 213, .. })
    ));
    assert!(matches!(
        Sprite::read(&file, tex_obj),
        Err(Error::WrongClass { found: 28, .. })
    ));
}

#[test]
fn data_in_outlives_the_texture_it_came_from() {
    let b = two_file_bundle(false, Limits::default());
    let a = Assets::from_bundle(b.clone(), "CAB-a").unwrap();
    // The Texture2D is a temporary here; the pixels borrow only the bundle.
    let pixels = a.texture(1).unwrap().data_in(&b).unwrap();
    assert_eq!(pixels.len(), 64);
}

#[test]
fn assets_open_from_bytes() {
    let file = file_2018(&[(
        7,
        TEXTURE_2D,
        rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4())),
    )]);
    let a = Assets::from_bytes(file.clone(), "", Limits::default()).unwrap();
    assert!(a.decode_texture(7).is_ok());
    let b = bundle(&BundleOpts::new(6, "2018.4.36f1"), &[("CAB-a", &file, 4)]);
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(a.decode_texture(7).is_ok());
}

#[test]
fn script_table_entries_are_aligned_even_when_the_table_is_not() {
    // With this version string and one MonoBehaviour type, the type table ends off a 4-byte
    // boundary; with no objects to realign it, each script's i64 needs the reader's padding.
    let extras = Extras {
        type_trees: false,
        mono: true,
        scripts: 2,
        externals: vec!["sharedassets1.assets".into()],
    };
    let bytes = serialized_with(22, "2022.3.9f1", false, 19, &[], &extras);
    let file = SerializedFile::parse(bytes).unwrap();
    let paths: Vec<&str> = file.externals().iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["sharedassets1.assets"]);
}

#[test]
fn an_undecodable_texture_is_refused_before_its_stream_is_read() {
    // The stream file does not exist: reaching for it would give an I/O error instead.
    let dir = TempDir::new("bc7");
    let px = Pixels::Streamed {
        path: "missing.resS",
        offset: 0,
        size: 16,
    };
    let obj = texture(Layout::U2018_4, false, "t", 4, 4, format::BC7, &px, &[]);
    let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
    match a.decode_texture(7) {
        Err(Error::UnsupportedTextureFormat {
            texture, format, ..
        }) => {
            assert_eq!((texture.as_deref(), format), (Some("t"), format::BC7));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(a.work_done(), 0);
}
