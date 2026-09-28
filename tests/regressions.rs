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
            crate_format::ARGB4444,
            crate_format::RGB565,
            crate_format::RGBA4444,
            crate_format::RGB24,
            crate_format::RGBA32,
            crate_format::ARGB32,
            crate_format::DXT1,
            crate_format::DXT5,
            crate_format::BGRA32
        ],
        [1, 2, 7, 13, 3, 4, 5, 10, 12, 14]
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
    let stream = |name: &'static str, second: bool| Pixels::Streamed {
        path: name,
        offset: if second && !same_range { 64 } else { 0 },
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
            &stream("archive:/CAB-a/CAB-a.resS", false),
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
            &stream("archive:/CAB-a/CAB-a.resS", true),
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
    // Decoding 16 pixels, masking, copying 16. The triangle (0,0) (4,0) (0,4) crosses rows
    // 0 to 3; each row's span, from where its sample lines (y + 0.25, y + 0.75) meet the
    // triangle widened a column each side and cut to the 4 columns, holds 4, 4, 3 and 2
    // columns. A step for each row and a test for each column: 17. In all, 49.
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::DEFAULT.with_max_total_work(49));
    assert!(a.export(&s).is_ok());
    assert_eq!(a.work_done(), 49);
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::DEFAULT.with_max_total_work(48));
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
    for name in ["COM10.resS", "CONSOLE.resS", "lpt.resS", "COMA.resS"] {
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

/// A 2022.3 texture with its `m_MipsStripped` and `m_MipCount` replaced.
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
    // From 2019.4.15 the header is padded even at format 6: read with the wrong rule, the
    // entry's bytes would not be the file's.
    let reads_file = |b: Result<Bundle, Error>| {
        b.is_ok_and(|b| b.entries().first().and_then(|e| b.bytes(e)) == Some(&file[..]))
    };
    assert!(reads_file(parse(6, "2019.4.15f1", 0, true)));
    assert!(reads_file(parse(6, "2019.4.14f1", 0, false)));
    // Only 2019 does this at format 6 (as UnityPy reads it); later releases write format 7.
    assert!(reads_file(parse(6, "2020.1.0f1", 0, false)));
    assert!(!reads_file(parse(6, "2020.1.0f1", 0, true)));
    assert!(!reads_file(parse(6, "2019.4.15f1", 0, false)));
    assert!(!reads_file(parse(6, "2019.4.14f1", 0, true)));
    assert!(!reads_file(parse(6, "2019.3.99f1", 0, true)));
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
            name: texture,
            format,
            ..
        }) => {
            assert_eq!((texture.as_deref(), format), (Some("t"), format::BC7));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(a.work_done(), 0);
}

// Round 5.

const fn streamed(path: &'static str, offset: u64, size: u32) -> Pixels<'static> {
    Pixels::Streamed { path, offset, size }
}

fn textures_on(paths: &[(&'static str, u64, u32)]) -> Vec<u8> {
    let objects: Vec<_> = paths
        .iter()
        .enumerate()
        .map(|(i, &(path, offset, size))| {
            let px = streamed(path, offset, size);
            (
                i as i64 + 1,
                TEXTURE_2D,
                rgba_texture(Layout::U2018_4, "t", 4, 4, &px),
            )
        })
        .collect();
    file_2018(&objects)
}

#[test]
fn every_spelling_of_a_bundle_stream_claims_the_same_bytes() {
    let file = textures_on(&[
        ("archive:/CAB-a/CAB-a.resS", 0, 64),
        ("CAB-a.resS", 0, 64),
        ("zzz/CAB-a.resS", 0, 64),
        ("archive:/CAB-other/CAB-a.resS", 32, 64),
        ("archive:/CAB-a/CAB-a.resS", 64, 64),
    ]);
    let b = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("CAB-a", &file, 4), ("CAB-a.resS", &[5; 128], 0)],
    );
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(a.decode_texture(1).is_ok());
    for alias in [2, 3, 4] {
        match a.decode_texture(alias) {
            Err(Error::Invalid(msg)) => assert!(msg.contains("already read"), "{msg}"),
            other => panic!("{alias}: {other:?}"),
        }
    }
    assert!(a.decode_texture(5).is_ok(), "the next range is fine");
    assert_eq!(a.work_done(), 32);
}

#[test]
fn a_stream_may_not_be_the_serialized_file() {
    let inline = rgba_texture(Layout::U2018_4, "a", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let px = streamed("archive:/CAB-a/CAB-a", 0, 64);
    let file = file_2018(&[
        (1, TEXTURE_2D, inline),
        (2, TEXTURE_2D, rgba_texture(Layout::U2018_4, "b", 4, 4, &px)),
    ]);
    let b = bundle(&BundleOpts::new(6, "2018.4.36f1"), &[("CAB-a", &file, 4)]);
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(a.decode_texture(1).is_ok());
    match a.decode_texture(2) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("serialized file"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_spelling_of_a_stream_file_claims_the_same_bytes() {
    let dir = TempDir::new("spellings");
    dir.file("t.resS", &[7; 64]);
    // Paths, not names, are refused outright.
    for name in ["t.resS/", "t.resS/.", "./t.resS", "t.resS//", "sub\\t.resS"] {
        let a = Assets::open(dir.file("p.assets", &textures_on(&[(name, 0, 64)]))).unwrap();
        assert!(
            matches!(a.decode_texture(1), Err(Error::Unsupported(_))),
            "{name}"
        );
    }
    // Another spelling of the same file, where the file system allows one (letter case on
    // macOS and Windows), is the same file: refused as a second read. Where it does not,
    // it names no file. Either way it is never read twice.
    let file = textures_on(&[("t.resS", 0, 64), ("T.resS", 0, 64)]);
    let a = Assets::open(dir.file("c.assets", &file)).unwrap();
    assert!(a.decode_texture(1).is_ok());
    match a.decode_texture(2) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("already read"), "{msg}"),
        Err(Error::Io { .. }) => {}
        other => panic!("{other:?}"),
    }
    // Two asset files beside one stream file each have their own claims.
    let other = Assets::open(dir.file("d.assets", &textures_on(&[("t.resS", 0, 64)]))).unwrap();
    assert!(other.decode_texture(1).is_ok());
}

#[test]
fn a_texture_that_fails_claims_nothing() {
    let dir = TempDir::new("failclaim");
    dir.file("t.resS", &[7; 64]);
    // Texture 1 names 128 bytes of a 64-byte file, texture 2 is too short for its pixels;
    // both fail, and texture 3 then reads the bytes they named.
    let file = textures_on(&[("t.resS", 0, 128), ("t.resS", 0, 63), ("t.resS", 0, 64)]);
    let a = Assets::open(dir.file("t.assets", &file)).unwrap();
    assert!(matches!(a.decode_texture(1), Err(Error::Invalid(_))));
    match a.decode_texture(2) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("holds 63 bytes"), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert!(a.decode_texture(3).is_ok());
    assert_eq!(a.work_done(), 16);
}

#[test]
fn short_pixel_data_is_refused_before_it_is_read() {
    // A 16384 x 16384 RGBA32 texture naming 1 GiB less one byte of a file that is not there:
    // refused for its length, not for the missing file.
    let dir = TempDir::new("short");
    let px = streamed("big.resS", 0, (1 << 30) - 1);
    let obj = rgba_texture(Layout::U2018_4, "t", 16384, 16384, &px);
    let a = Assets::open(dir.file("t.assets", &file_2018(&[(1, TEXTURE_2D, obj)]))).unwrap();
    match a.decode_texture(1) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("needs 1073741824"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // Inline data too.
    let obj = rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&[1; 60]));
    let a = Assets::open(dir.file("u.assets", &file_2018(&[(1, TEXTURE_2D, obj)]))).unwrap();
    assert!(matches!(a.decode_texture(1), Err(Error::Invalid(_))));
    assert_eq!(a.work_done(), 0);
}

#[test]
fn export_tries_a_failing_texture_once() {
    let dir = TempDir::new("once");
    let r = [0.0, 0.0, 1.0, 1.0];
    let on = || {
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
            RECT,
            1.0,
            &Mesh::BASE,
        )
    };
    let px = streamed("late.resS", 0, 64);
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (
                TEX,
                TEXTURE_2D,
                rgba_texture(Layout::U2022_3, "t", 4, 4, &px),
            ),
            (1, SPRITE, on()),
            (2, SPRITE, on()),
        ],
    );
    let mut a = Assets::open(dir.file("s.assets", &file)).unwrap();
    let list = a.sprites(|_| true);
    let first = a.export(&list.sprites[0]).unwrap_err();
    assert!(
        matches!(&first, Error::TextureUnreadable { path_id: TEX, .. }
            if first.io_error().map(std::io::Error::kind) == Some(std::io::ErrorKind::NotFound)
                && matches!(first.root(), Error::Io { .. })),
        "{first:?}"
    );
    // The file appears; the failure is remembered until the cache is cleared.
    dir.file("late.resS", &[3; 64]);
    assert!(a.export(&list.sprites[1]).is_err());
    a.clear_cache();
    assert!(a.export(&list.sprites[1]).is_ok());
}

#[test]
fn a_mask_refused_for_the_total_is_never_built() {
    // A tight 4x4 sprite: decode 16, mask 17, copy 16. At a total of 48 the mask and copy
    // are refused together, before either starts, and nothing is charged for them.
    let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::DEFAULT.with_max_total_work(48));
    for _ in 0..3 {
        assert!(matches!(
            a.export(&s),
            Err(Error::LimitExceeded {
                kind: LimitKind::TotalWork,
                ..
            })
        ));
    }
    assert_eq!(a.work_done(), 16, "the decode only");
}

/// The smallest `max_decompressed` under which `bundle` parses.
fn least_decompression_limit(bundle: &[u8]) -> u64 {
    let parses =
        |limit| Bundle::parse_with(bundle, Limits::DEFAULT.with_max_decompressed(limit)).is_ok();
    let (mut lo, mut hi) = (0u64, 1 << 30);
    assert!(parses(hi));
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if parses(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

// The charges below are in-memory sizes on a 64-bit target.
#[cfg(target_pointer_width = "64")]
#[test]
fn the_directory_and_lzma_working_memory_are_charged() {
    let data: Vec<u8> = (0..4000u32).map(|i| (i * 7 % 251) as u8).collect();
    let entries = [("a.resS", &data[..], 0)];
    let with = |blocks: Vec<u16>| {
        let mut o = BundleOpts::new(6, "2018.4.36f1");
        o.blocks = blocks;
        bundle(&o, &entries)
    };
    // Stored: the directory's bytes (16-byte hash, block count, 10 bytes a block, entry count,
    // 20 bytes and the path an entry), 24 bytes a parsed block, 298 an entry (itself, its
    // share of two hash tables, and allocator overhead on three copies of its path) and three
    // times its path, and the data.
    let info = 16 + 4 + 10 + 4 + 20 + 7;
    let stored = least_decompression_limit(&with(vec![0]));
    assert_eq!(stored, info + 24 + 4000 + 298 + 3 * 6);
    // LZMA adds its tables (lc 3, lp 0: 2 * 0x300 << 3 bytes, plus 4 KiB) for every block,
    // and twice the largest block for the dictionary.
    let tables = 2 * (0x300 << 3) + 4096;
    let one = least_decompression_limit(&with(vec![1]));
    // (The dictionary never counts as less than lzma-rs's 4 KiB floor.)
    assert_eq!(one, stored + tables + 2 * 4096);
    let four = least_decompression_limit(&with(vec![1; 4]));
    assert_eq!(four, stored + 3 * 24 + 3 * 10 + 4 * tables + 2 * 4096);
}

#[test]
fn lzma_properties_outside_lzma2s_bounds_are_refused() {
    let data = [9u8; 300];
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![1];
    let good = bundle(&o, &[("a.resS", &data, 0)]);
    assert!(Bundle::parse(&good).is_ok());
    // lzma-rs writes lc 3, lp 0, pb 2 (0x5d) and an 8 MiB dictionary.
    let at = good
        .windows(5)
        .position(|w| w == [0x5d, 0, 0, 0x80, 0])
        .unwrap();
    // lc 4, lp 0 is the most LZMA2 allows; lc 5 and lp 1 with lc 4 are over.
    for (props, ok) in [
        (4 + 2 * 45, true),
        (5 + 2 * 45, false),
        (4 + 9 + 2 * 45, false),
        (3 + 5 * 45, false),
    ] {
        let mut b = good.clone();
        b[at] = props;
        let parsed = Bundle::parse(&b);
        if ok {
            // A header that is merely unusual may still decode wrongly; it is not refused.
            assert!(!matches!(parsed, Err(Error::Unsupported(_))), "{props}");
        } else {
            match parsed {
                Err(Error::Unsupported(msg)) => assert!(msg.contains("LZMA"), "{msg}"),
                other => panic!("{props}: {other:?}"),
            }
        }
    }
}

#[test]
fn strings_are_held_to_4_kib() {
    let long = |n: usize| "n".repeat(n);
    for (n, ok) in [(4096, true), (4097, false)] {
        let obj = rgba_texture(
            Layout::U2018_4,
            &long(n),
            4,
            4,
            &Pixels::Inline(&rgba_4x4()),
        );
        let file = SerializedFile::parse(file_2018(&[(7, TEXTURE_2D, obj)])).unwrap();
        let o = &file.objects()[0];
        assert_eq!(file.name(o).is_some(), ok, "{n}");
        assert_eq!(Texture2D::read(&file, o).is_ok(), ok, "{n}");
    }
    // Bundle entry paths too.
    for (n, ok) in [(4096, true), (4097, false)] {
        let b = bundle(&BundleOpts::new(6, "2018.4.36f1"), &[(&long(n), &[1], 0)]);
        assert_eq!(Bundle::parse(&b).is_ok(), ok, "{n}");
    }
}

#[test]
fn sprites_must_end_where_their_fields_do() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let base = || {
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
            RECT,
            1.0,
            &Mesh::BASE,
        )
    };
    let skipped = |version: &str, s: Vec<u8>| {
        let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
        let file = serialized(
            22,
            version,
            false,
            19,
            &[(TEX, TEXTURE_2D, tex), (1, SPRITE, s)],
        );
        let file = SerializedFile::parse(file).unwrap();
        let a = Assets::from_serialized(file, "").unwrap();
        a.sprites(|_| true).skipped.len()
    };
    assert_eq!(skipped("2022.3.62f1", base()), 0);
    let mut extra = base();
    extra.extend([0; 4]);
    assert_eq!(skipped("2022.3.62f1", extra), 1);
    // Bones gain a GUID and a colour in 2021.1; m_ScriptableObjects follows from 2023.1.
    assert_eq!(skipped("2022.3.62f1", with_bone(base(), true)), 0);
    assert_eq!(skipped("2022.3.62f1", with_bone(base(), false)), 1);
    assert_eq!(skipped("2020.3.48f1", with_bone(base(), false)), 0);
    assert_eq!(skipped("2020.3.48f1", with_bone(base(), true)), 1);
    assert_eq!(skipped("2023.1.20f1", from_2023(base())), 0);
    assert_eq!(skipped("2023.1.20f1", base()), 1);
}

#[test]
fn sprite_sub_meshes_may_not_share_indices() {
    let twice = Mesh {
        repeat_submesh: true,
        ..TRIANGLE
    };
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, tight(&twice, 0))],
    );
    let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    let list = a.sprites(|_| true);
    match &list.skipped[..] {
        [s] => assert!(s.error.to_string().contains("share indices"), "{}", s.error),
        other => panic!("{other:?}"),
    }
}

#[test]
fn windows_device_names_are_refused_however_spelled() {
    let dir = TempDir::new("devices");
    for name in [
        "CON .resS",
        "nul..resS",
        "Com1  .resource",
        "LPT\u{b9}.resS",
        "COM0.resS",
        "lpt0.resource",
        "caf\u{e9}.resS",
        "\u{17f}.resS",
        "conin$ .resS",
        "AUX.x.resS",
    ] {
        let obj = texture(
            Layout::U2018_4,
            false,
            "t",
            4,
            4,
            format::ALPHA8,
            &streamed(name, 0, 16),
            &[],
        );
        let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
        assert!(
            matches!(a.decode_texture(7), Err(Error::Unsupported(_))),
            "{name:?}"
        );
    }
}

#[test]
fn atlases_are_read_only_when_a_sprite_needs_one() {
    let r = [0.0, 0.0, 2.0, 2.0];
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let atlased = sprite(
        false,
        false,
        "a",
        r,
        [0.0, 0.0],
        5,
        30,
        0,
        0,
        r,
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let mut objects = vec![
        (TEX, TEXTURE_2D, tex),
        (30, SPRITE_ATLAS, atlas(false, &[(5, TEX, r, RECT, 1.0)])),
    ];
    // Atlases no sprite names, some of them empty objects.
    objects.extend((100..200).map(|i| (i, SPRITE_ATLAS, Vec::new())));
    objects.push((1, SPRITE, atlased));
    let file = serialized(22, "2022.3.62f1", false, 19, &objects);
    let mut a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    assert!(format!("{a:?}").contains("atlases: 0"), "{a:?}");
    let list = a.sprites(|_| true);
    assert!(a.export(&list.sprites[0]).is_ok());
    assert!(format!("{a:?}").contains("atlases: 1"), "{a:?}");
}

const DXT1_5X3: &str = "bd926bff7ba639ffa7985affbd926bff000000007ba639ff919f49ff919f49ffbd926bffb1e5acffbd926bffbd926bffa7985affa7985affbdd7ffff";
const DXT5_EQUAL_ALPHA_IN: &str = "808088462cb66ddb8dbc277da03d2427";
const DXT5_EQUAL_ALPHA_OUT: &str = "919f49007ba63900a7985a00bd926b00bd926b007ba63900a7985a00bd926b007ba63980919f4980919f4980bd926b80bd926b80bd926b80a7985a80a7985a80";

#[test]
fn dxt_edges_match_the_reference_decoder() {
    // A 5x3 texture: two blocks side by side, cropped (Pillow's output, rows flipped).
    let blocks = &hex(DXT1_IN)[..16];
    assert_eq!(
        decode::decode(format::DXT1, 5, 3, blocks).unwrap(),
        hex(DXT1_5X3)
    );
    // Equal alpha endpoints select the six-value palette with 0 and 255.
    let out = decode::decode(format::DXT5, 4, 4, &hex(DXT5_EQUAL_ALPHA_IN)).unwrap();
    assert_eq!(out, hex(DXT5_EQUAL_ALPHA_OUT));
    // Nothing to decode is nothing, not a panic.
    for fmt in [format::DXT1, format::DXT5, format::RGBA32, format::ALPHA8] {
        assert_eq!(decode::decode(fmt, 0, 4, &[]).map_or(0, |v| v.len()), 0);
        assert_eq!(decode::decode(fmt, 4, 0, &[]).map_or(0, |v| v.len()), 0);
    }
}

#[test]
fn the_mesh_offset_moves_it_up_as_well_as_across() {
    // Full rect 4x5 with the 4x4 texture rect one pixel up; pivot (0.75, 0.4), 100 pixels per
    // unit: pixel = vertex * 100 + (4 * 0.75 - 0, 5 * 0.4 - 1) = vertex * 100 + (3, 1).
    let mesh = Mesh {
        vertices: &[[-0.03, -0.01], [0.01, -0.01], [-0.03, 0.03]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let s = sprite_full(
        false,
        false,
        "s",
        [0.0, 0.0, 4.0, 5.0],
        [0.75, 0.4],
        1,
        0,
        TEX,
        0,
        [0.0, 0.0, 4.0, 4.0],
        0,
        1.0,
        &mesh,
        100.0,
        [0.0, 1.0],
    );
    let (_d, mut a, s) = one_sprite(s, Limits::default());
    assert_eq!(
        kept(&a.export(&s).unwrap()),
        [12, 8, 9, 4, 5, 6, 0, 1, 2, 3]
    );
}

#[test]
fn fractional_rects_snap_then_round_outwards() {
    // (x, width) -> the pixels cut: noise within a thousandth snaps to the pixel; more rounds
    // the rect outwards to whole pixels.
    for (x, width, want) in [
        (1.0004, 2.0, 2),
        (0.9996, 2.0, 2),
        (1.002, 2.0, 3),
        (0.4, 2.2, 3),
    ] {
        let r = [x, 0.0, width, 1.0];
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
        let (_d, mut a, s) = one_sprite(s, Limits::default());
        let got = a.export(&s).map_or(0, |img| img.width);
        assert_eq!(got, want, "x {x}, width {width}");
    }
}

#[test]
fn every_console_platform_is_refused_and_others_are_not() {
    // Unity's BuildTarget numbers: PS3, Xbox 360, PS Vita, PS4, Xbox One, 3DS, Wii U, Switch,
    // Xbox Series, Xbox One (GameCore), PS5.
    let consoles = [10, 11, 30, 31, 33, 35, 36, 38, 42, 43, 44];
    let dir = TempDir::new("platforms");
    for platform in -2..=48 {
        let obj = rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
        let file = serialized(17, "2018.4.36f1", false, platform, &[(7, TEXTURE_2D, obj)]);
        let a = Assets::open(dir.file("t.assets", &file)).unwrap();
        let refused = matches!(a.decode_texture(7), Err(Error::Unsupported(_)));
        assert_eq!(refused, consoles.contains(&platform), "platform {platform}");
        assert_eq!(unity_bundle_assets::is_console_platform(platform), refused);
    }
}

#[test]
fn threads_share_one_budget_and_one_set_of_claims() {
    // Eight textures over one stream range, and eight with their own; 16 pixels each.
    let dir = TempDir::new("threads");
    dir.file("t.resS", &[7; 64 * 9]);
    let mut paths: Vec<(&'static str, u64, u32)> = vec![("t.resS", 0, 64); 8];
    paths.extend((1..=8).map(|i| ("t.resS", 64 * i, 64)));
    let file = textures_on(&paths);
    let path = dir.file("t.assets", &file);
    // Room for five decodes.
    let a = Arc::new(Assets::open_with(&path, Limits::DEFAULT.with_max_total_work(80)).unwrap());
    let results: Vec<(i64, bool)> = std::thread::scope(|scope| {
        #[allow(
            clippy::needless_collect,
            reason = "all threads start before any is joined"
        )]
        let handles: Vec<_> = (1..=16)
            .map(|id| {
                let a = a.clone();
                scope.spawn(move || (id, a.decode_texture(id).is_ok()))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let decoded: Vec<i64> = results.iter().filter(|r| r.1).map(|r| r.0).collect();
    let shared = decoded.iter().filter(|&&id| id <= 8).count();
    assert!(shared <= 1, "one range decoded {shared} times: {decoded:?}");
    assert_eq!(decoded.len(), 5, "{decoded:?}");
    assert_eq!(a.work_done(), 80);
}

// Round 5: gaps the mutation run found.

#[test]
fn cut_names_the_image_it_was_given() {
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
    let (_d, a, s) = one_sprite(s, Limits::default());
    // An image built wrongly by hand: fields are public, so its length can disagree.
    let mut image = Image::new(3, 2, vec![0; 24]).unwrap();
    image.rgba.pop();
    match a.cut(&s, &image) {
        Err(Error::Invalid(msg)) => {
            assert!(msg.contains("3x2 image holds 23 bytes, not 24"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bundle_of_many_files_lists_five() {
    let file = file_2018(&[]);
    for (n, listed, more) in [(2, 2, false), (5, 5, false), (6, 5, true), (7, 5, true)] {
        let names: Vec<String> = (0..n).map(|i| format!("CAB-{i}")).collect();
        let entries: Vec<(&str, &[u8], u32)> =
            names.iter().map(|n| (n.as_str(), &file[..], 4)).collect();
        let b = bundle(&BundleOpts::new(6, "2018.4.36f1"), &entries);
        match Assets::from_bytes(b, "", Limits::default()) {
            Err(Error::Unsupported(msg)) => {
                assert!(
                    msg.contains(&format!("holds {n} serialized files")),
                    "{msg}"
                );
                assert!(msg.contains(&format!("\"CAB-{}\"", listed - 1)), "{msg}");
                assert!(!msg.contains(&format!("\"CAB-{listed}\"")), "{msg}");
                assert_eq!(msg.contains(", ..."), more, "{msg}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_name_that_cannot_be_read_is_offered_to_the_filter_as_empty() {
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, Vec::new())],
    );
    let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    // Filtered out like any other name the filter refuses; kept, then skipped as unreadable.
    assert!(a.sprites(|n| n == "Hero").skipped.is_empty());
    let list = a.sprites(str::is_empty);
    assert_eq!(list.skipped.len(), 1, "{list:?}");
    assert!(list.skipped[0].name.is_none());
    // Textures do the same.
    let names: Vec<_> = a.textures(|_| true).into_iter().map(|t| t.name).collect();
    assert_eq!(names, [Some("t".to_string())]);
}

#[test]
fn sprites_are_ordered_by_the_texture_that_holds_them() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let tex = || rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let in_atlas = |key, atlas_file: i32, own: i64| {
        let mut s = sprite(
            false,
            false,
            "s",
            r,
            [0.0, 0.0],
            key,
            30,
            own,
            0,
            r,
            RECT,
            1.0,
            &Mesh::BASE,
        );
        // Point m_SpriteAtlas at another file: its PPtr is (file 0, 30) after the key.
        let key_bytes = key.to_le_bytes();
        let at = s.windows(8).position(|w| w == key_bytes).unwrap() + 8 + 4; // key, tags count
        s[at..at + 4].copy_from_slice(&atlas_file.to_le_bytes());
        s
    };
    // The local atlas 30 puts keys 1 and 3 in texture 10, key 4 in texture 11.
    let atl = atlas(
        false,
        &[
            (1, 10, r, RECT, 1.0),
            (3, 10, r, RECT, 1.0),
            (4, 11, r, RECT, 1.0),
        ],
    );
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (10, TEXTURE_2D, tex()),
            (11, TEXTURE_2D, tex()),
            (30, SPRITE_ATLAS, atl),
            // In file order: key 4 (atlas: texture 11), key 1 (an atlas in another file, so
            // its own texture 11 counts, not the local atlas's 10), key 3 (atlas: texture 10).
            (100, SPRITE, in_atlas(4, 0, 0)),
            (101, SPRITE, in_atlas(1, 1, 11)),
            (102, SPRITE, in_atlas(3, 0, 0)),
        ],
    );
    let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    let order: Vec<i64> = a
        .sprites(|_| true)
        .sprites
        .iter()
        .map(|s| s.path_id)
        .collect();
    assert_eq!(order, [102, 100, 101]);
}

#[test]
fn a_one_pixel_wide_texture_is_not_empty() {
    let obj = rgba_texture(Layout::U2018_4, "t", 1, 4, &Pixels::Inline(&[5; 16]));
    let a = Assets::from_bytes(file_2018(&[(7, TEXTURE_2D, obj)]), "", Limits::default()).unwrap();
    assert_eq!(a.decode_texture(7).unwrap().rgba, vec![5; 16]);
    let obj = rgba_texture(Layout::U2018_4, "t", 4, 1, &Pixels::Inline(&[5; 16]));
    let a = Assets::from_bytes(file_2018(&[(7, TEXTURE_2D, obj)]), "", Limits::default()).unwrap();
    assert!(a.decode_texture(7).is_ok());
}

#[test]
fn a_stream_offset_that_overflows_is_an_error() {
    // 2020.1 and later store the offset in 64 bits.
    let px = streamed("CAB-a.resS", u64::MAX - 8, 64);
    let obj = rgba_texture(Layout::U2022_3, "t", 4, 4, &px);
    let file = serialized(22, "2022.3.62f1", false, 19, &[(1, TEXTURE_2D, obj)]);
    let b = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("CAB-a", &file, 4), ("CAB-a.resS", &[5; 128], 0)],
    );
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(matches!(a.decode_texture(1), Err(Error::Invalid(_))));
}

#[test]
fn stream_entries_are_claimed_where_they_are_in_the_bundle() {
    // Two stream entries back to back; the first texture reads all of the first, the second
    // the second half of the second. Different bytes, so both decode.
    let file = textures_on(&[("first.resS", 0, 64), ("second.resS", 64, 64)]);
    let b = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[
            ("CAB-a", &file, 4),
            ("first.resS", &[1; 64], 0),
            ("second.resS", &[2; 128], 0),
        ],
    );
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(a.decode_texture(1).is_ok());
    assert!(a.decode_texture(2).is_ok());
}

#[test]
fn format_versions_outside_17_to_22_are_told_apart_from_other_files() {
    let unsupported =
        |data: Vec<u8>| matches!(SerializedFile::parse(data), Err(Error::Unsupported(_)));
    let not_unity = |data: Vec<u8>| matches!(SerializedFile::parse(data), Err(Error::NotUnity(_)));
    // Older Unity files state their size where v17 does; newer ones leave that field 0.
    assert!(unsupported(serialized(16, "5.4.6f3", false, 19, &[])));
    assert!(unsupported(serialized(9, "3.4.0f5", false, 19, &[])));
    assert!(not_unity(serialized(8, "3.4.0f5", false, 19, &[])));
    assert!(unsupported(serialized(23, "6000.4.2f1", false, 19, &[])));
    assert!(unsupported(serialized(40, "6000.4.2f1", false, 19, &[])));
    assert!(not_unity(serialized(41, "6000.4.2f1", false, 19, &[])));
    let mut sized = serialized(23, "6000.4.2f1", false, 19, &[]);
    sized[4..8].copy_from_slice(&1u32.to_be_bytes());
    assert!(not_unity(sized));
    let mut resized = serialized(16, "5.4.6f3", false, 19, &[]);
    resized[4..8].copy_from_slice(&1u32.to_be_bytes());
    assert!(not_unity(resized));
}

#[test]
fn counts_that_exactly_fit_are_not_bad_lengths() {
    // A count as large as the bytes left could hold at the real minimum size is read on (and
    // fails later, for want of real records); one more is a bad length.
    fn fit(data: &mut [u8], at: usize, min_size: usize, extra: u32) {
        let n = ((data.len() - at - 4) / min_size) as u32 + extra;
        data[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    // Whether the count at `at` is the one refused (a later count, read from the records that
    // are not there, may be refused too).
    let bad_length = |d: Vec<u8>, at: usize| matches!(SerializedFile::parse(d), Err(Error::BadLength { at: refused, .. }) if refused == at);
    // Enough bytes after each count that one byte more or less in the minimum size changes
    // what fits.
    let tex = rgba_texture(Layout::U2018_4, "t", 64, 16, &Pixels::Inline(&[1; 4096]));
    for (version, unity, object_size) in [(17, "2018.4.36f1", 20), (22, "2022.3.62f1", 24)] {
        let file = serialized(version, unity, false, 19, &[(7, TEXTURE_2D, tex.clone())]);
        let header = if version >= 22 { 48 } else { 20 };
        let types_at = header + unity.len() + 1 + 4 + 1;
        let objects_at = types_at + 4 + 23;
        for (at, size) in [(types_at, 23), (objects_at, object_size)] {
            let mut exact = file.clone();
            fit(&mut exact, at, size, 0);
            assert!(!bad_length(exact, at), "v{version} at {at}: {size}");
            let mut over = file.clone();
            fit(&mut over, at, size, 1);
            assert!(bad_length(over, at), "v{version} at {at}: {size}");
        }
    }
}

#[test]
fn objects_overlap_by_their_bytes() {
    // (path ID, bytes): 1 and 2 are one byte each; 3 is empty; 4 is four bytes.
    let objects = [
        (1, TEXTURE_2D, vec![1]),
        (2, TEXTURE_2D, vec![2]),
        (3, TEXTURE_2D, Vec::new()),
        (4, TEXTURE_2D, vec![4; 4]),
    ];
    let file = file_2018(&objects);
    // Each object's entry: path ID (8), start (4), size (4), type (4).
    let start_of =
        |data: &[u8], id: i64| data.windows(8).position(|w| w == id.to_le_bytes()).unwrap() + 8;
    let first_start = file[start_of(&file, 1)..start_of(&file, 1) + 4].to_vec();
    // Object 2 moved onto object 1: two one-byte objects over one byte.
    let mut onto = file.clone();
    let at = start_of(&onto, 2);
    onto[at..at + 4].copy_from_slice(&first_start);
    match SerializedFile::parse(onto) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("objects 1 and 2 overlap"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // The empty object moved into the middle of object 4: it holds no bytes, so it overlaps
    // nothing.
    let mut empty = file;
    let fourth = start_of(&empty, 4);
    let inside = u32::from_le_bytes(empty[fourth..fourth + 4].try_into().unwrap()) + 2;
    let at = start_of(&empty, 3);
    empty[at..at + 4].copy_from_slice(&inside.to_le_bytes());
    assert!(SerializedFile::parse(empty).is_ok());
}

#[test]
fn rects_are_whole_pixels_in_both_directions() {
    // (x, y, width, height) -> the pixels cut, or refused for covering none.
    type Case = ([f32; 4], Option<(u32, u32)>);
    let cases: [Case; 7] = [
        ([0.0, 1.0004, 2.0, 2.0], Some((2, 2))),
        ([0.0, 1.002, 2.0, 2.0], Some((2, 3))),
        ([0.0, 0.4, 2.0, 2.2], Some((2, 3))),
        ([1.0, 0.0, 0.0, 2.0], None),
        ([1.0, 1.0, 2.0, 0.0], None),
        ([f32::NAN, 0.0, 2.0, 2.0], None),
        ([0.0, 0.0, 2.0, f32::INFINITY], None),
    ];
    for (r, want) in cases {
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
        let (_d, mut a, s) = one_sprite(s, Limits::default());
        match (a.export(&s), want) {
            (Ok(img), Some(size)) => assert_eq!((img.width, img.height), size, "{r:?}"),
            (Err(Error::Invalid(_)), None) => {}
            (other, _) => panic!("{r:?}: {other:?}"),
        }
    }
}

#[test]
fn large_coordinates_snap_by_their_own_precision() {
    // At 16000 a float's step is about 0.001, so a rect there carries noise that large; it
    // still snaps to the whole pixel (the tolerance grows with the value).
    let tex = rgba_texture(
        Layout::U2022_3,
        "t",
        16384,
        1,
        &Pixels::Inline(&vec![3; 16384 * 4]),
    );
    let x = 16000.004_f32;
    assert!((x - 16000.0).abs() > 1e-3);
    let r = [x, 0.0, 2.0, 1.0];
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
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, s)],
    );
    let mut a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    let list = a.sprites(|_| true);
    assert_eq!(a.export(&list.sprites[0]).unwrap().width, 2);
}

#[test]
fn a_downscale_within_a_ten_thousandth_counts_as_none() {
    let r = [0.0, 0.0, 2.0, 2.0];
    for (downscale, ok) in [
        (1.0, true),
        (1.000_05, true),
        (0.999_95, true),
        (1.000_5, false),
        (0.5, false),
    ] {
        let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
        let s = sprite(
            false,
            false,
            "s",
            r,
            [0.0, 0.0],
            5,
            30,
            0,
            0,
            r,
            RECT,
            1.0,
            &Mesh::BASE,
        );
        let atl = atlas(false, &[(5, TEX, r, RECT, downscale)]);
        let file = serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[
                (TEX, TEXTURE_2D, tex),
                (30, SPRITE_ATLAS, atl),
                (1, SPRITE, s),
            ],
        );
        let mut a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
        let list = a.sprites(|_| true);
        assert_eq!(a.export(&list.sprites[0]).is_ok(), ok, "{downscale}");
    }
}

/// A tight sprite over an 8x8 texture with `mesh`, its vertices in pixels.
fn tight_8x8(mesh: &Mesh, limits: Limits) -> (TempDir, Assets, Sprite) {
    let dir = TempDir::new("tight8");
    let px: Vec<u8> = (0..64u8).flat_map(|i| [i * 4, 0, 0, 255]).collect();
    let tex = rgba_texture(Layout::U2022_3, "t", 8, 8, &Pixels::Inline(&px));
    let r = [0.0, 0.0, 8.0, 8.0];
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
        0,
        1.0,
        mesh,
    );
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, s)],
    );
    let a = Assets::open_with(dir.file("t.assets", &file), limits).unwrap();
    let s = a.sprites(|_| true).sprites.remove(0);
    (dir, a, s)
}

#[test]
fn mask_work_is_each_rows_span_and_nothing_for_a_flat_triangle() {
    // A triangle (1,2) (5,2) (1,7), and a flat one along a diagonal, which has no area and
    // costs nothing. The triangle crosses rows 2 to 6; its long edge is x = 5 - 0.8 (y - 2),
    // so each row's span (its sample lines' crossings, a column wider each side, cut to the
    // box's columns 1 to 4) holds 4, 4, 3, 3 and 2 columns: with a step a row, 21. Decode 64,
    // mask 21, copy 64.
    let triangle = Mesh {
        vertices: &[
            [1.0, 2.0],
            [5.0, 2.0],
            [1.0, 7.0],
            [2.0, 3.0],
            [4.0, 5.0],
            [6.0, 7.0],
        ],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let with_flat = Mesh {
        indices: &[0, 1, 2, 3, 4, 5],
        ..triangle
    };
    for mesh in [&triangle, &with_flat] {
        let (_d, mut a, s) = tight_8x8(mesh, Limits::default());
        a.export(&s).unwrap();
        assert_eq!(a.work_done(), 64 + 21 + 64);
    }
    // The span is the whole charge: one less and it is refused.
    let (_d, mut a, s) = tight_8x8(&with_flat, Limits::DEFAULT.with_max_mask_work(20));
    assert!(matches!(
        a.export(&s),
        Err(Error::LimitExceeded {
            kind: LimitKind::MaskWork,
            ..
        })
    ));
}

#[test]
fn a_thin_triangle_costs_its_height_not_its_box() {
    // 4096 slivers from (0,0) to (64,32), each a hundredth of a pixel thick at its end: each
    // box is 64 x 32 = 2048 pixels, 8M in all, but each row's span is a few columns.
    let vertices: &[[f32; 2]] = &[[0.0, 0.0], [64.0, 32.0], [64.0, 32.01]];
    let indices: Vec<u16> = std::iter::repeat_n([0u16, 1, 2], 4096).flatten().collect();
    let mesh = Mesh {
        vertices,
        indices: &indices,
        ..Mesh::BASE
    };
    let dir = TempDir::new("slivers");
    let tex = rgba_texture(
        Layout::U2022_3,
        "t",
        64,
        64,
        &Pixels::Inline(&[9; 64 * 64 * 4]),
    );
    let r = [0.0, 0.0, 64.0, 64.0];
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
        0,
        1.0,
        &mesh,
    );
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, s)],
    );
    let mut a = Assets::open(dir.file("t.assets", &file)).unwrap();
    let s = a.sprites(|_| true).sprites.remove(0);
    a.export(&s).unwrap();
    let mask = a.work_done() - 2 * 64 * 64;
    assert!(mask < 4096 * 33 * 6, "{mask}");
}

#[test]
fn a_sample_on_an_edge_is_inside_whichever_way_the_triangle_winds() {
    // Corners at quarter points of three pixels, (0,0), (3,0) and (0,3): in each, the corner
    // is the only sample the triangle reaches, and it lies on two edges.
    let expected: Vec<u8> = (0..8u8)
        .rev()
        .flat_map(|y| (0..8u8).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            [0.25f32, 0.75].iter().any(|&u| {
                [0.25f32, 0.75].iter().any(|&v| {
                    let (px, py) = (f32::from(x) + u, f32::from(y) + v);
                    px >= 0.75 && py >= 0.75 && px + py <= 4.0
                })
            })
        })
        .map(|(x, y)| y * 8 + x)
        .collect();
    assert!(expected.contains(&0) && expected.contains(&3) && expected.contains(&24));
    for indices in [[0, 1, 2], [0, 2, 1]] {
        let mesh = Mesh {
            vertices: &[[0.75, 0.75], [3.25, 0.75], [0.75, 3.25]],
            indices: &indices,
            ..Mesh::BASE
        };
        let (_d, mut a, s) = tight_8x8(&mesh, Limits::default());
        assert_eq!(kept(&a.export(&s).unwrap()), expected, "{indices:?}");
    }
}

#[test]
fn every_vertex_format_has_its_size() {
    // A channel after the position in the same stream sets the stride; read with the wrong
    // component size, every vertex after the first is misplaced.
    let baseline = {
        let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::default());
        kept(&a.export(&s).unwrap())
    };
    for format in 0..=11u8 {
        for dims in [0, 1, 2, 3] {
            let mesh = Mesh {
                extra_channel: Some((format, dims)),
                ..TRIANGLE
            };
            let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
            assert_eq!(
                kept(&a.export(&s).unwrap()),
                baseline,
                "format {format} x {dims}"
            );
        }
        // UVs first, in their own stream: their size sets where the positions start.
        let mesh = Mesh {
            uv_format: format,
            pos_stream: 1,
            ..TRIANGLE
        };
        let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
        assert_eq!(kept(&a.export(&s).unwrap()), baseline, "UV format {format}");
    }
    // An unknown format cannot be sized: the mesh is not read.
    let mesh = Mesh {
        extra_channel: Some((12, 2)),
        ..TRIANGLE
    };
    let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
    assert!(matches!(a.export(&s), Err(Error::Unsupported(_))));
}

#[test]
fn positions_of_two_to_four_components_are_read() {
    let baseline = {
        let (_d, mut a, s) = one_sprite(tight(&TRIANGLE, 0), Limits::default());
        kept(&a.export(&s).unwrap())
    };
    // Two components, in the last stream: the last vertex ends where the data does.
    for (dims, pos_stream) in [(2, 1), (2, 0), (3, 1), (4, 0)] {
        let mesh = Mesh {
            pos_dims: dims,
            pos_stream,
            ..TRIANGLE
        };
        let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
        assert_eq!(
            kept(&a.export(&s).unwrap()),
            baseline,
            "{dims} in stream {pos_stream}"
        );
    }
    for dims in [1, 5] {
        let mesh = Mesh {
            pos_dims: dims,
            ..TRIANGLE
        };
        let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
        assert!(matches!(a.export(&s), Err(Error::Unsupported(_))), "{dims}");
    }
}

#[test]
fn an_index_one_past_the_last_vertex_spoils_the_mesh() {
    // The UV stream follows the positions, so the bytes past the last vertex exist.
    let mesh = Mesh {
        indices: &[0, 1, 3],
        ..TRIANGLE
    };
    let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
    assert!(matches!(a.export(&s), Err(Error::Unsupported(_))));
}

#[test]
fn sub_meshes_may_touch_but_not_share() {
    // Six indices: the main sub-mesh takes the first three (bytes 0..6).
    let base = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0], [4.0, 4.0]],
        indices: &[0, 1, 2, 1, 3, 2],
        index_count: Some(3),
        ..Mesh::BASE
    };
    for (extra, ok) in [
        ((6, 3, 0), true),  // the next three
        ((4, 3, 0), false), // one index shared
        ((4, 1, 0), false), // one index, shared
        ((2, 0, 0), true),  // none
        ((0, 3, 3), true),  // lines, which are not read
    ] {
        let mesh = Mesh {
            extra_submeshes: &[extra],
            ..base
        };
        let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
        let file = serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(TEX, TEXTURE_2D, tex), (1, SPRITE, tight(&mesh, 0))],
        );
        let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
        assert_eq!(a.sprites(|_| true).skipped.is_empty(), ok, "{extra:?}");
    }
    // Four indices take eight bytes: a sub-mesh from byte 6 shares the fourth.
    let mesh = Mesh {
        index_count: Some(4),
        extra_submeshes: &[(6, 3, 0)],
        ..base
    };
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, tex), (1, SPRITE, tight(&mesh, 0))],
    );
    let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    assert_eq!(a.sprites(|_| true).skipped.len(), 1);
}

#[test]
fn a_sprites_triangles_are_its_indices_in_threes() {
    let two = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0], [4.0, 4.0]],
        indices: &[0, 1, 2, 1, 3, 2],
        ..Mesh::BASE
    };
    for (limit, ok) in [(2, true), (1, false)] {
        let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
        let file = serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(TEX, TEXTURE_2D, tex), (1, SPRITE, tight(&two, 0))],
        );
        let file =
            SerializedFile::parse_with(file, Limits::DEFAULT.with_max_sprite_triangles(limit))
                .unwrap();
        let a = Assets::from_serialized(file, "").unwrap();
        assert_eq!(a.sprites(|_| true).skipped.is_empty(), ok, "{limit}");
    }
}

#[test]
fn a_mesh_that_cannot_be_read_takes_nothing_from_the_total() {
    // Sprite 1's positions are integers (unreadable); its one triangle is counted against
    // the total before that is known, but none are kept, so sprite 2's triangle still fits.
    let unreadable = Mesh {
        pos_format: 10,
        ..TRIANGLE
    };
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (TEX, TEXTURE_2D, tex),
            (1, SPRITE, tight(&unreadable, 0)),
            (2, SPRITE, tight(&TRIANGLE, 0)),
        ],
    );
    let file =
        SerializedFile::parse_with(file, Limits::DEFAULT.with_max_total_triangles(1)).unwrap();
    let a = Assets::from_serialized(file, "").unwrap();
    let list = a.sprites(|_| true);
    assert_eq!((list.sprites.len(), list.skipped.len()), (2, 0), "{list:?}");
}

#[test]
fn sprite_lists_are_read_and_stepped_over() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let with = |extras: &SpriteExtras| {
        sprite_ext(
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
            1.0,
            [0.0, 0.0],
            extras,
        )
    };
    let outline: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    for extras in [
        SpriteExtras::default(),
        SpriteExtras {
            atlas_tags: &["tag", "second tag"],
            ..Default::default()
        },
        SpriteExtras {
            secondary_textures: &[(11, "_NormalMap"), (12, "_MaskTex")],
            ..Default::default()
        },
        SpriteExtras {
            bindposes: 2,
            ..Default::default()
        },
        SpriteExtras {
            outlines: &[outline, &outline[..2]],
            ..Default::default()
        },
    ] {
        let (_d, mut a, s) = one_sprite(with(&extras), Limits::default());
        assert!(a.export(&s).is_ok());
    }
}

#[test]
fn atlases_step_over_their_packed_sprite_lists() {
    let r = [0.0, 0.0, 2.0, 2.0];
    for name in ["a", "ab", "abc", "abcd", "abcde"] {
        let atl = atlas_full(
            false,
            true,
            name,
            &[(1, "s"), (2, "another")],
            &[(5, TEX, r, RECT, 1.0)],
        );
        let file = SerializedFile::parse(serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(30, SPRITE_ATLAS, atl)],
        ))
        .unwrap();
        let read = unity_bundle_assets::SpriteAtlas::read(&file, &file.objects()[0]);
        assert!(read.is_ok(), "{name}: {read:?}");
    }
}

#[test]
fn atlas_entries_gain_secondary_textures_in_2020_2() {
    let r = [0.0, 0.0, 2.0, 2.0];
    for (unity, secondary, ok) in [
        ("2020.1.17f1", false, true),
        ("2020.1.17f1", true, false),
        ("2020.2.0f1", true, true),
        ("2020.2.0f1", false, false),
    ] {
        let atl = atlas_with(false, secondary, &[(5, TEX, r, RECT, 1.0)]);
        let file =
            SerializedFile::parse(serialized(22, unity, false, 19, &[(30, SPRITE_ATLAS, atl)]))
                .unwrap();
        let read = unity_bundle_assets::SpriteAtlas::read(&file, &file.objects()[0]);
        assert_eq!(read.is_ok(), ok, "{unity} {secondary}: {read:?}");
    }
}

#[test]
fn sprite_gates_at_their_first_releases() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let base = || {
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
            RECT,
            1.0,
            &Mesh::BASE,
        )
    };
    let skipped = |unity: &str, s: Vec<u8>| {
        let file = serialized(22, unity, false, 19, &[(1, SPRITE, s)]);
        let a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
        a.sprites(|_| true)
            .skipped
            .into_iter()
            .next()
            .map(|s| s.error.to_string())
    };
    // Sprites are read from 2019.1.
    assert_eq!(skipped("2019.1.0f1", base()), None);
    let old = skipped("2018.4.36f1", base()).unwrap();
    assert!(old.contains("needs 2019.1"), "{old}");
    let new = skipped("6000.5.0f1", base()).unwrap();
    assert!(new.contains("up to 6000.4"), "{new}");
    // Bones gain a GUID and a colour at 2021.1.0; m_ScriptableObjects arrives at 2023.1.0.
    for unity in ["2021.1.0f1", "2021.3.40f1"] {
        assert_eq!(skipped(unity, with_bone(base(), true)), None, "{unity}");
        assert!(
            skipped(unity, with_bone(base(), false)).is_some(),
            "{unity}"
        );
    }
    assert!(skipped("2020.3.48f1", with_bone(base(), true)).is_some());
    assert_eq!(skipped("2023.1.0f1", from_2023(base())), None);
    assert_eq!(skipped("2023.1.0f1", from_2023_with(base(), 3)), None);
    assert!(skipped("2023.1.0f1", base()).is_some());
    assert_eq!(skipped("2022.3.62f1", base()), None);
}

#[test]
fn rotation_bits_count_only_when_packed() {
    // Settings with a flip in the rotation bits but no packed bit: the sprite is not turned.
    let r = [0.0, 0.0, 4.0, 4.0];
    let export = |settings: u32| {
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
            settings,
            1.0,
            &Mesh::BASE,
        );
        let (_d, mut a, s) = one_sprite(s, Limits::default());
        a.export(&s).unwrap().rgba
    };
    assert_eq!(export(RECT | 1 << 2), export(RECT));
    assert_ne!(export(RECT | PACKED | 1 << 2), export(RECT));
}

#[test]
fn a_texture_refused_its_budget_gives_its_range_back() {
    let dir = TempDir::new("giveback");
    dir.file("t.resS", &[7; 256]);
    // Texture 1 wants 64 pixels of the range, texture 2 16 pixels of the same range.
    let big = rgba_texture(Layout::U2018_4, "big", 8, 8, &streamed("t.resS", 0, 256));
    let small = rgba_texture(Layout::U2018_4, "small", 4, 4, &streamed("t.resS", 0, 64));
    let file = file_2018(&[(1, TEXTURE_2D, big), (2, TEXTURE_2D, small)]);
    let a = Assets::open_with(
        dir.file("t.assets", &file),
        Limits::DEFAULT.with_max_total_work(20),
    )
    .unwrap();
    assert!(matches!(
        a.decode_texture(1),
        Err(Error::LimitExceeded { .. })
    ));
    assert!(a.decode_texture(2).is_ok());
}

#[test]
fn engine_versions_of_32_characters_are_the_longest() {
    let v32 = format!("2018.4.36f1-{}", "x".repeat(20));
    assert_eq!(v32.len(), 32);
    let v33 = format!("{v32}x");
    assert!(SerializedFile::parse(serialized(17, &v32, false, 19, &[])).is_ok());
    assert!(matches!(
        SerializedFile::parse(serialized(17, &v33, false, 19, &[])),
        Err(Error::NotUnity(_))
    ));
}

#[test]
fn a_one_byte_stream_is_a_stream() {
    let dir = TempDir::new("onebyte");
    dir.file("t.resS", &[200]);
    let obj = texture(
        Layout::U2018_4,
        false,
        "t",
        1,
        1,
        format::ALPHA8,
        &streamed("t.resS", 0, 1),
        &[],
    );
    let a = Assets::open(dir.file("t.assets", &file_2018(&[(7, TEXTURE_2D, obj)]))).unwrap();
    assert_eq!(a.decode_texture(7).unwrap().rgba, [255, 255, 255, 200]);
}

const DXT1_4X5: &str = "bdd7ffffb1e5acff00000000b1e5acff919f49ff7ba639ffa7985affbd926bffbd926bff7ba639ffa7985affbd926bff7ba639ff919f49ff919f49ffbd926bffbd926bffbd926bffa7985affa7985aff";

#[test]
fn a_texture_five_rows_high_takes_two_block_rows() {
    let blocks = &hex(DXT1_IN)[..16];
    assert_eq!(decode::mip0_size(format::DXT1, 4, 5), Some(16));
    assert_eq!(decode::mip0_size(format::DXT1, 4, 8), Some(16));
    assert_eq!(decode::mip0_size(format::DXT1, 4, 9), Some(24));
    assert_eq!(
        decode::decode(format::DXT1, 4, 5, blocks).unwrap(),
        hex(DXT1_4X5)
    );
    let obj = texture(
        Layout::U2018_4,
        false,
        "t",
        4,
        5,
        format::DXT1,
        &Pixels::Inline(blocks),
        &[],
    );
    let a = Assets::from_bytes(file_2018(&[(7, TEXTURE_2D, obj)]), "", Limits::default()).unwrap();
    assert_eq!(a.decode_texture(7).unwrap().rgba, hex(DXT1_4X5));
}

#[test]
fn bundle_entries_that_overflow_or_touch() {
    let opts = BundleOpts::new(6, "2018.4.36f1");
    let good = bundle(
        &opts,
        &[
            ("a.resS", &[1; 40], 0),
            ("b.resS", &[2; 1], 0),
            ("c.resS", &[], 0),
        ],
    );
    assert!(Bundle::parse(&good).is_ok());
    let entry = |data: &[u8], offset: i64, size: i64| {
        let pat = [offset.to_be_bytes(), size.to_be_bytes()].concat();
        data.windows(16).position(|w| w == pat).unwrap()
    };
    // An offset near the top of i64 overflows when its size is added.
    let mut b = good.clone();
    let at = entry(&b, 40, 1);
    b[at..at + 8].copy_from_slice(&(i64::MAX - 1).to_be_bytes());
    assert!(matches!(Bundle::parse(&b), Err(Error::Invalid(_))));
    // The empty entry inside the first: fine. The one-byte entry inside it: overlapping.
    let mut b = good.clone();
    let at = entry(&b, 41, 0);
    b[at..at + 8].copy_from_slice(&5i64.to_be_bytes());
    assert!(Bundle::parse(&b).is_ok());
    let mut b = good;
    let at = entry(&b, 40, 1);
    b[at..at + 8].copy_from_slice(&5i64.to_be_bytes());
    assert!(matches!(Bundle::parse(&b), Err(Error::Invalid(_))));
}

#[test]
fn flag_0x200_before_2020_is_encryption() {
    let file = file_2018(&[]);
    let mut o = BundleOpts::new(6, "2019.4.40f1");
    o.extra_flags = 0x200;
    o.align_header = true;
    assert!(matches!(
        Bundle::parse(&bundle(&o, &[("CAB-a", &file, 4)])),
        Err(Error::Encrypted)
    ));
}

#[test]
fn lzma_blocks_with_pb_4_and_short_headers() {
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![1];
    let good = bundle(&o, &[("a.resS", &[9; 300], 0)]);
    let at = good
        .windows(5)
        .position(|w| w == [0x5d, 0, 0, 0x80, 0])
        .unwrap();
    // pb 4 is allowed (it only changes how the stream decodes).
    let mut b = good.clone();
    b[at] = 3 + 4 * 45;
    assert!(!matches!(Bundle::parse(&b), Err(Error::Unsupported(_))));
    // A block of fewer than five bytes has no header.
    let mut info = good.clone();
    let compressed = (good.len() - at) as u32;
    let size_at = good
        .windows(8)
        .position(|w| w[..4] == 300u32.to_be_bytes() && w[4..] == compressed.to_be_bytes())
        .unwrap();
    info[size_at + 4..size_at + 8].copy_from_slice(&3u32.to_be_bytes());
    info.truncate(at + 3);
    fix_bundle_size(&mut info);
    match Bundle::parse(&info) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("5-byte header"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

// UnityPy's output (Pillow's raw decoders) for 4x2 pixels of these bytes, rows flipped.
const WORDS_4X2: &str = "a54dca182530bb1d6d132cded6237b2e";
const ARGB4444_OUT: &str = "3366dd11ee22ccdd33dd6622ee77bb22ddaa554488ccaa1100225533ddbbbb11";
const RGB565_OUT: &str = "106d6affdec662ff2079b4ff29cedeff4ab629ff181852ff310429ff18b6deff";
const RGBA4444_OUT: &str = "113366ddddee22cc2233dd6622ee77bb44ddaa551188ccaa3300225511ddbbbb";

#[test]
fn sixteen_bit_formats_match_the_reference_decoder() {
    for (fmt, out) in [
        (format::ARGB4444, ARGB4444_OUT),
        (format::RGB565, RGB565_OUT),
        (format::RGBA4444, RGBA4444_OUT),
    ] {
        assert_eq!(
            decode::decode(fmt, 4, 2, &hex(WORDS_4X2)).unwrap(),
            hex(out),
            "{fmt}"
        );
        assert_eq!(decode::mip0_size(fmt, 4, 2), Some(16));
    }
    // Every level of each channel: 0 and the top level are 0 and 255, and levels round down.
    let all: Vec<u8> = (0..=u16::MAX).flat_map(u16::to_le_bytes).collect();
    let rgb = decode::decode(format::RGB565, 256, 256, &all).unwrap();
    let levels = |channel: usize| {
        let mut v: Vec<u8> = rgb.chunks(4).map(|p| p[channel]).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let expect = |max: u32| {
        (0..=max)
            .map(|c| (c * 255 / max) as u8)
            .collect::<Vec<u8>>()
    };
    assert_eq!(levels(0), expect(31));
    assert_eq!(levels(1), expect(63));
    assert_eq!(levels(2), expect(31));
}

// Round 6.

fn one_texture_file() -> Vec<u8> {
    let t = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&[3; 64]));
    serialized(22, "2022.3.62f1", false, 19, &[(10, TEXTURE_2D, t)])
}

#[test]
fn newer_bundles_with_compressed_directories() {
    // From 2020.3.34 (and with a stripped version) flag 0x200 is padding and 0x1400 is
    // encryption; a compressed directory must still parse.
    for info in [1u32, 2, 3] {
        for (format, revision) in [(8, "2022.3.62f1"), (7, "2020.3.34f1"), (6, "0.0.0")] {
            let mut o = BundleOpts::new(format, revision);
            o.info = info;
            o.blocks = vec![0];
            o.padding_flag = format == 8;
            let b = bundle(&o, &[("CAB-a", &one_texture_file(), 4)]);
            let parsed = Bundle::parse(&b);
            assert!(
                parsed.is_ok(),
                "info {info}, {revision}: {:?}",
                parsed.err()
            );
        }
    }
}

#[test]
fn a_bundles_declared_size_rules_open_and_parse_alike() {
    let dir = TempDir::new("declared");
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![0];
    let good = bundle(&o, &[("CAB-a", &one_texture_file(), 4)]);
    let at_size = |b: &[u8]| {
        let mut i = 12;
        for _ in 0..2 {
            while b[i] != 0 {
                i += 1;
            }
            i += 1;
        }
        i
    };
    let with_size = |size: i64, tail: usize| {
        let mut b = good.clone();
        let at = at_size(&b);
        b[at..at + 8].copy_from_slice(&size.to_be_bytes());
        b.extend(vec![0xee; tail]);
        b
    };
    let len = good.len() as i64;
    // (declared size, bytes after the bundle, parses)
    for (size, tail, ok) in [
        (len, 0, true),
        (len, 100, true), // what follows the declared size is not the bundle's
        (0, 0, false),
        (-1, 0, false),
        (100, 0, false),
        (len + 1, 0, false),
        (i64::MAX, 0, false),
    ] {
        let b = with_size(size, tail);
        let parsed = Bundle::parse(&b).is_ok();
        let opened = Bundle::open(dir.file("x.bundle", &b)).is_ok();
        let assets = Assets::from_bytes(b.clone(), "", Limits::default()).is_ok();
        assert_eq!(
            (parsed, opened, assets),
            (ok, ok, ok),
            "declared {size}, {tail} after"
        );
    }
}

#[test]
fn objects_must_reach_their_final_padding() {
    // A stream path of 6 bytes is followed by 2 bytes of padding; without them the texture
    // ends inside its own alignment.
    let t = rgba_texture(Layout::U2022_3, "t", 4, 4, &streamed("a.resS", 0, 64));
    let mut short = t.clone();
    short.truncate(t.len() - 2);
    for (object, ok) in [(t, true), (short, false)] {
        let file = SerializedFile::parse(serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(10, TEXTURE_2D, object)],
        ))
        .unwrap();
        assert_eq!(Texture2D::read(&file, &file.objects()[0]).is_ok(), ok);
    }
    let a = atlas(false, &[(5, TEX, [0.0, 0.0, 1.0, 1.0], RECT, 1.0)]);
    let mut short = a.clone();
    short.truncate(a.len() - 3);
    for (object, ok) in [(a, true), (short, false)] {
        let file = SerializedFile::parse(serialized(
            22,
            "2022.3.62f1",
            false,
            19,
            &[(30, SPRITE_ATLAS, object)],
        ))
        .unwrap();
        let read = unity_bundle_assets::SpriteAtlas::read(&file, &file.objects()[0]);
        assert_eq!(read.is_ok(), ok, "{read:?}");
    }
}

#[test]
fn a_stream_entry_with_the_directory_bit_is_still_a_stream() {
    let file = textures_on(&[("archive:/CAB-a/CAB-a.resS", 0, 64)]);
    let b = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("CAB-a", &file, 4), ("CAB-a.resS", &[1; 64], 1)],
    );
    let a = Assets::from_bytes(b, "", Limits::default()).unwrap();
    assert!(a.decode_texture(1).is_ok());
}

#[test]
fn racing_threads_never_share_a_range() {
    // Room for the big texture once (64) and the small one (16), not the big one twice.
    let dir = TempDir::new("race");
    dir.file("t.resS", &[7; 256]);
    let big = rgba_texture(Layout::U2018_4, "big", 8, 8, &streamed("t.resS", 0, 256));
    let small = rgba_texture(Layout::U2018_4, "small", 4, 4, &streamed("t.resS", 0, 64));
    let path = dir.file(
        "t.assets",
        &file_2018(&[(1, TEXTURE_2D, big), (2, TEXTURE_2D, small)]),
    );
    for _ in 0..2000 {
        let a = Assets::open_with(&path, Limits::DEFAULT.with_max_total_work(100)).unwrap();
        let barrier = std::sync::Barrier::new(4);
        let decoded = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        a.decode_texture(1).is_ok()
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .filter(|&ok| ok)
                .count()
        });
        assert_eq!(decoded, 1);
        assert!(
            a.decode_texture(2).is_err(),
            "the range was read by a second texture"
        );
    }
}

#[test]
fn a_texture_refused_its_budget_keeps_the_range_it_read() {
    let dir = TempDir::new("keep");
    dir.file("t.resS", &[7; 256]);
    let big = rgba_texture(Layout::U2018_4, "big", 8, 8, &streamed("t.resS", 0, 256));
    let small = rgba_texture(Layout::U2018_4, "small", 4, 4, &streamed("t.resS", 0, 64));
    let file = file_2018(&[(1, TEXTURE_2D, big), (2, TEXTURE_2D, small)]);
    let a = Assets::open_with(
        dir.file("t.assets", &file),
        Limits::DEFAULT.with_max_total_work(80),
    )
    .unwrap();
    assert!(a.decode_texture(1).is_ok());
    assert!(matches!(
        a.decode_texture(1),
        Err(Error::LimitExceeded { .. })
    ));
    assert!(
        a.decode_texture(2).is_err(),
        "range read by texture 1 was handed to texture 2"
    );
}

#[test]
fn a_stream_conflict_names_the_file() {
    let dir = TempDir::new("owner");
    dir.file("t.resS", &[7; 64]);
    let p = dir.file(
        "x.assets",
        &textures_on(&[("t.resS", 0, 64), ("t.resS", 0, 64)]),
    );
    let a = Assets::open(&p).unwrap();
    a.decode_texture(1).unwrap();
    let e = a.decode_texture(2).unwrap_err().to_string();
    assert!(
        e.contains("x.assets") && e.contains("texture 2") && e.contains("texture 1"),
        "{e}"
    );
}

#[test]
fn from_bytes_holds_to_the_file_size_limit() {
    let r = Assets::from_bytes(
        one_texture_file(),
        "",
        Limits::DEFAULT.with_max_file_size(10),
    );
    assert!(
        matches!(
            r,
            Err(Error::LimitExceeded {
                kind: LimitKind::FileSize,
                ..
            })
        ),
        "{:?}",
        r.err()
    );
}

#[test]
fn an_atlas_in_another_file_is_not_looked_up_here() {
    // The sprite's atlas is (file 1, 30); this file's own atlas 30 holds the sprite's key and
    // must not be used.
    let r = [0.0, 0.0, 2.0, 2.0];
    let mut s = sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        5,
        30,
        0,
        0,
        r,
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let at = s
        .windows(12)
        .position(|w| w == [0, 0, 0, 0, 30, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    s[at..at + 4].copy_from_slice(&1i32.to_le_bytes());
    let tex = rgba_texture(Layout::U2022_3, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let atl = atlas(false, &[(5, TEX, r, RECT, 1.0)]);
    let extras = Extras {
        externals: vec!["other.assets".into()],
        ..Extras::default()
    };
    let file = serialized_with(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (TEX, TEXTURE_2D, tex),
            (1, SPRITE, s),
            (30, SPRITE_ATLAS, atl),
        ],
        &extras,
    );
    let mut a = Assets::from_serialized(SerializedFile::parse(file).unwrap(), "").unwrap();
    let list = a.sprites(|_| true);
    assert_eq!(list.sprites[0].atlas.file_id, 1);
    assert!(matches!(
        a.export(&list.sprites[0]),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn a_sprite_with_no_atlas_and_no_texture_has_no_texture() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let s = sprite(
        false,
        false,
        "s",
        r,
        [0.0, 0.0],
        1,
        0,
        0,
        0,
        r,
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let f = serialized(22, "2022.3.62f1", false, 19, &[(1, SPRITE, s)]);
    let a = Assets::from_serialized(SerializedFile::parse(f).unwrap(), "").unwrap();
    let list = a.sprites(|_| true);
    // Its placement is its own render data (no atlas to look up); that names no texture.
    assert!(a.placement(&list.sprites[0]).is_ok());
    match a.texture_id(&list.sprites[0]) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("has no texture"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_rect_as_wide_as_the_snapping_tolerance_covers_nothing() {
    let r = [0.0, 0.0, 4.0, 4.0];
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
        [0.0, 0.0, 0.001, 1.0],
        RECT,
        1.0,
        &Mesh::BASE,
    );
    let (_d, mut a, s) = one_sprite(s, Limits::default());
    match a.export(&s) {
        Err(Error::Invalid(msg)) => assert!(msg.contains("no whole pixel"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_triangle_of_area_one_billionth_is_kept() {
    // Area exactly 1e-9 (as f32): not flat, so the sprite exports (fully transparent).
    let mesh = Mesh {
        vertices: &[[0.0, 0.0], [1e-9, 0.0], [0.0, 1.0]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let (_d, mut a, s) = one_sprite(tight(&mesh, 0), Limits::default());
    let img = a.export(&s).unwrap();
    assert!(img.rgba.iter().all(|&b| b == 0));
    // Area below it is flat: no triangle with area, an error.
    let flat = Mesh {
        vertices: &[[0.0, 0.0], [1e-10, 0.0], [0.0, 1.0]],
        ..mesh
    };
    let (_d, mut a, s) = one_sprite(tight(&flat, 0), Limits::default());
    assert!(matches!(a.export(&s), Err(Error::Invalid(_))));
}

#[test]
fn every_object_in_a_bundle_counts_toward_one_limit() {
    let tex = || rgba_texture(Layout::U2018_4, "t", 4, 4, &Pixels::Inline(&rgba_4x4()));
    let a = file_2018(&[(1, TEXTURE_2D, tex()), (2, TEXTURE_2D, tex())]);
    let b = file_2018(&[(1, TEXTURE_2D, tex()), (2, TEXTURE_2D, tex())]);
    let bytes = bundle(
        &BundleOpts::new(6, "2018.4.36f1"),
        &[("CAB-a", &a, 4), ("CAB-b", &b, 4)],
    );
    for (limit, second) in [(4, true), (3, false)] {
        let bundle =
            Arc::new(Bundle::parse_with(&bytes, Limits::DEFAULT.with_max_objects(limit)).unwrap());
        assert!(Assets::from_bundle(bundle.clone(), "CAB-a").is_ok());
        let opened = Assets::from_bundle(bundle, "CAB-b");
        assert_eq!(opened.is_ok(), second, "{limit}: {:?}", opened.err());
    }
}

#[test]
fn files_too_short_for_a_header_are_not_unity() {
    let dir = TempDir::new("tiny");
    for bytes in [&b""[..], b"hello", &[0; 19]] {
        let p = dir.file("tiny", bytes);
        assert!(
            matches!(Assets::open(&p), Err(Error::NotUnity(_))),
            "{bytes:?}"
        );
        assert!(
            matches!(
                SerializedFile::parse(bytes.to_vec()),
                Err(Error::NotUnity(_))
            ),
            "{bytes:?}"
        );
    }
}

#[test]
fn error_messages_quote_at_most_64_characters_of_a_name() {
    let name = "\u{1}".repeat(4000);
    let obj = rgba_texture(Layout::U2018_4, &name, 4, 4, &Pixels::Inline(&[1; 60]));
    let a = Assets::from_bytes(file_2018(&[(7, TEXTURE_2D, obj)]), "", Limits::default()).unwrap();
    let msg = a.decode_texture(7).unwrap_err().to_string();
    assert!(msg.len() < 700, "{}", msg.len());
    assert!(msg.contains("... (4000 bytes)"), "{msg}");
}

#[test]
fn the_root_error_is_the_same_through_export() {
    let dir = TempDir::new("root");
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
    let bc7 = texture(
        Layout::U2022_3,
        false,
        "t",
        4,
        4,
        format::BC7,
        &Pixels::Inline(&[0; 16]),
        &[],
    );
    let f = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(TEX, TEXTURE_2D, bc7), (1, SPRITE, s)],
    );
    let mut a = Assets::open(dir.file("t.assets", &f)).unwrap();
    let direct = a.decode_texture(TEX).unwrap_err();
    let list = a.sprites(|_| true);
    let exported = a.export(&list.sprites[0]).unwrap_err();
    assert!(matches!(
        exported,
        Error::TextureUnreadable { path_id: TEX, .. }
    ));
    for e in [&direct, &exported] {
        assert!(
            matches!(e.root(), Error::UnsupportedTextureFormat { format: 25, .. }),
            "{e:?}"
        );
        assert!(e.io_error().is_none());
    }
}

#[test]
fn an_lzma_directory_is_held_to_the_same_rules_as_its_blocks() {
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.info = 1;
    o.blocks = vec![0];
    let good = bundle(&o, &[("CAB-a", &one_texture_file(), 4)]);
    assert!(Bundle::parse(&good).is_ok());
    // lzma-rs writes lc 3, lp 0, pb 2 (0x5d) and an 8 MiB dictionary; the directory is the
    // first LZMA stream in the file.
    let at = good
        .windows(5)
        .position(|w| w == [0x5d, 0, 0, 0x80, 0])
        .unwrap();
    let mut wide = good.clone();
    wide[at] = 8 + 4 * 9 + 2 * 45; // lc 8, lp 4: 6 MiB of tables
    assert!(matches!(Bundle::parse(&wide), Err(Error::Unsupported(msg)) if msg.contains("LZMA")));
    // Its tables and dictionary count against the decompression limit like a block's.
    let least = least_decompression_limit(&good);
    let mut stored = o;
    stored.info = 0;
    assert!(
        least
            > least_decompression_limit(&bundle(&stored, &[("CAB-a", &one_texture_file(), 4)]))
                + 16_000
    );
}

#[test]
fn an_lzma_directory_is_charged_its_tables_and_dictionary() {
    // A small bundle whose LZMA directory's working memory outweighs everything else: its
    // 60 bytes, tables for lc 3, lp 0 (2 * 0x300 << 3, plus 4 KiB) and a dictionary counted at
    // twice lzma-rs's 4 KiB floor.
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.info = 1;
    o.blocks = vec![0];
    let b = bundle(&o, &[("CAB-a", &[1; 8], 4)]);
    let info = 16 + 4 + 10 + 4 + 20 + 6;
    assert_eq!(
        least_decompression_limit(&b),
        info + (2 * (0x300 << 3) + 4096) + 2 * 4096
    );
}

#[test]
fn an_lzma_dictionary_counts_as_at_least_4_kib() {
    // A 10,000-byte block: with an 8 MiB dictionary it is charged 2 * 10,000; with a 16-byte
    // one, lzma-rs still uses 4 KiB, so 2 * 4096.
    let data: Vec<u8> = (0..10_000u32).map(|i| (i % 7) as u8).collect();
    let mut o = BundleOpts::new(6, "2018.4.36f1");
    o.blocks = vec![1];
    let big = bundle(&o, &[("a.resS", &data, 0)]);
    let at = big
        .windows(5)
        .position(|w| w == [0x5d, 0, 0, 0x80, 0])
        .unwrap();
    let mut small = big.clone();
    small[at + 1..at + 5].copy_from_slice(&16u32.to_le_bytes());
    assert!(Bundle::parse(&small).is_ok());
    assert_eq!(
        least_decompression_limit(&big) - least_decompression_limit(&small),
        2 * 10_000 - 2 * 4096
    );
}

#[test]
fn twenty_bytes_are_enough_to_be_read_as_a_header() {
    // Nineteen are too short to say what they are; twenty are read, and are not Unity.
    for (len, short) in [(19, true), (20, false)] {
        match SerializedFile::parse(vec![0; len]) {
            Err(Error::NotUnity(msg)) => assert_eq!(msg.contains("too short"), short, "{msg}"),
            other => panic!("{len}: {other:?}"),
        }
    }
}

#[test]
fn atlas_entries_step_over_their_secondary_textures() {
    // Every entry carries two secondary textures from 2020.2.
    let r = [0.0, 0.0, 2.0, 2.0];
    let atl = atlas_full(
        false,
        true,
        "atlas",
        &[],
        &[(5, TEX, r, RECT, 1.0), (6, TEX, r, RECT, 1.0)],
    );
    let file = SerializedFile::parse(serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[(30, SPRITE_ATLAS, atl)],
    ))
    .unwrap();
    let read = unity_bundle_assets::SpriteAtlas::read(&file, &file.objects()[0]).unwrap();
    assert_eq!(read.entries().len(), 2);
}

/// Which pixels of a `w` x `h` image the triangles cover by the documented rule, tested pixel
/// by pixel over the whole image: kept when any quarter point lies on or inside a triangle.
fn reference_mask(triangles: &[[[f32; 2]; 3]], w: u32, h: u32) -> Vec<bool> {
    let inside = |t: &[[f32; 2]; 3], px: f32, py: f32| {
        let side =
            |p: [f32; 2], q: [f32; 2]| (q[0] - p[0]) * (py - p[1]) - (q[1] - p[1]) * (px - p[0]);
        let d = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
        !(d.iter().any(|&v| v < 0.0) && d.iter().any(|&v| v > 0.0))
    };
    let mut out = Vec::new();
    for y in (0..h).rev() {
        for x in 0..w {
            let hit = triangles.iter().any(|t| {
                let area = (t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
                    - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]);
                area.abs() >= 1e-9
                    && [0.25f32, 0.75].iter().any(|&u| {
                        [0.25f32, 0.75]
                            .iter()
                            .any(|&v| inside(t, x as f32 + u, y as f32 + v))
                    })
            });
            out.push(hit);
        }
    }
    out
}

#[test]
fn masks_match_a_whole_image_test_of_every_pixel() {
    // Random triangles over an 8x8 sprite, many with corners on quarter points and edges
    // along the sample lines, where rounding in the row spans would show.
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..400 {
        let point = |rnd: &mut dyn FnMut() -> u64| {
            let quarter = |r: u64| (r % 41) as f32 * 0.25 - 1.0;
            let v = [quarter(rnd()), quarter(rnd())];
            if rnd() % 4 == 0 {
                [v[0] + (rnd() % 1000) as f32 * 1e-4, v[1]]
            } else {
                v
            }
        };
        let mut triangles = Vec::new();
        let mut verts = Vec::new();
        for _ in 0..=(rnd() % 3) {
            let mut t = [point(&mut rnd), point(&mut rnd), point(&mut rnd)];
            if rnd() % 3 == 0 {
                t[1][1] = t[0][1]; // an edge along a row
            }
            triangles.push(t);
            verts.extend(t);
        }
        let indices: Vec<u16> = (0..verts.len() as u16).collect();
        let mesh = Mesh {
            vertices: &verts,
            indices: &indices,
            ..Mesh::BASE
        };
        let want = reference_mask(&triangles, 8, 8);
        let (_d, mut a, s) = tight_8x8(&mesh, Limits::default());
        match a.export(&s) {
            Ok(img) => {
                let got: Vec<bool> = img.rgba.chunks(4).map(|p| p[3] != 0).collect();
                assert_eq!(got, want, "round {round}: {triangles:?}");
            }
            // No triangle with area: nothing for the reference to keep either.
            Err(Error::Invalid(_)) => assert!(want.iter().all(|&k| !k), "round {round}"),
            Err(e) => panic!("round {round}: {e}"),
        }
    }
}
