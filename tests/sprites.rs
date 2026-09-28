//! Sprites end to end: placement, packing rotation, tight masks, atlases, and the ways each can
//! fail, on files built byte by byte (see `common`).
//!
//! The texture is 4x4 RGBA32 with pixel `i` (stored order, bottom row first) holding red
//! `4 * i`, so an exported image reads back as the stored indices of the pixels it took.

mod common;
use common::*;
use unity_bundle_assets::{Assets, Error, Image, LimitKind, Limits};

const TEX: i64 = 10;
const ATLAS: i64 = 30;
/// `SpriteSettings`: packed, rectangle packing (not tight), rotation in bits 2-5.
const RECT: u32 = 0b10;
const PACKED: u32 = 0b01;

fn ids(image: &Image) -> Vec<u8> {
    image.rgba.chunks(4).map(|p| p[0] / 4).collect()
}

fn texture_object(big: bool) -> Vec<u8> {
    texture(
        if big {
            Layout::U2019_4
        } else {
            Layout::U2022_3
        },
        big,
        "tex",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    )
}

const NO_MESH: Mesh = Mesh::BASE;

/// A sprite on the test texture with its own render data.
fn own(name: &str, rect: [f32; 4], settings: u32) -> Vec<u8> {
    sprite(
        false,
        false,
        name,
        rect,
        [0.0, 0.0],
        1,
        0,
        TEX,
        0,
        rect,
        settings,
        1.0,
        &NO_MESH,
    )
}

fn open_with(objects: &[(i64, i32, Vec<u8>)], unity: &str, limits: Limits) -> (TempDir, Assets) {
    let dir = TempDir::new("sprites");
    let version = if unity.starts_with("2019") { 21 } else { 22 };
    let path = dir.file("s.assets", &serialized(version, unity, false, 19, objects));
    let assets = Assets::open_with(&path, limits).unwrap();
    (dir, assets)
}

fn open(objects: &[(i64, i32, Vec<u8>)]) -> (TempDir, Assets) {
    open_with(objects, "2022.3.62f1", Limits::default())
}

fn export_one(assets: &mut Assets, name: &str) -> Result<Image, Error> {
    let list = assets.sprites(|n| n == name);
    if let Some(s) = list.skipped.into_iter().next() {
        return Err(s.error);
    }
    let sprite = list.sprites.into_iter().next().expect("sprite listed");
    assets.export(&sprite)
}

fn with_texture(sprites: Vec<(i64, Vec<u8>)>) -> Vec<(i64, i32, Vec<u8>)> {
    let mut objects = vec![(TEX, TEXTURE_2D, texture_object(false))];
    objects.extend(sprites.into_iter().map(|(id, data)| (id, SPRITE, data)));
    objects
}

#[test]
fn rect_sprite_crops_top_row_first() {
    let (_d, mut a) = open(&with_texture(vec![(
        1,
        own("s", [1.0, 1.0, 2.0, 2.0], RECT),
    )]));
    let img = export_one(&mut a, "s").unwrap();
    assert_eq!((img.width, img.height), (2, 2));
    assert_eq!(ids(&img), [9, 10, 5, 6]);
}

#[test]
fn packing_rotations_are_undone() {
    // AssetStudio's convention; UnityPy turns Rotate90 the other way.
    for (rot, want) in [
        (0u32, vec![9, 10, 5, 6]),
        (1, vec![10, 9, 6, 5]), // flipped horizontally
        (2, vec![5, 6, 9, 10]), // flipped vertically
        (3, vec![6, 5, 10, 9]), // half turn
        (4, vec![5, 9, 6, 10]), // quarter turn
    ] {
        let s = own("s", [1.0, 1.0, 2.0, 2.0], PACKED | RECT | rot << 2);
        let (_d, mut a) = open(&with_texture(vec![(1, s)]));
        assert_eq!(
            ids(&export_one(&mut a, "s").unwrap()),
            want,
            "rotation {rot}"
        );
    }
    // A non-square quarter turn swaps the dimensions.
    let s = own("s", [0.0, 0.0, 3.0, 2.0], PACKED | RECT | 4 << 2);
    let (_d, mut a) = open(&with_texture(vec![(1, s)]));
    let img = export_one(&mut a, "s").unwrap();
    assert_eq!((img.width, img.height), (2, 3));
    assert_eq!(ids(&img), [0, 4, 1, 5, 2, 6]);
}

#[test]
fn rects_are_snapped_then_must_fit() {
    // Float noise does not add a row.
    let (_d, mut a) = open(&with_texture(vec![(
        1,
        own("s", [1.0, 1.00003, 2.0, 2.0], RECT),
    )]));
    let img = export_one(&mut a, "s").unwrap();
    assert_eq!((img.width, img.height), (2, 2));
    // Anything else outside the texture, or empty, is an error rather than a smaller image.
    for rect in [
        [3.0, 3.0, 2.0, 2.0],
        [-1.0, 0.0, 2.0, 2.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 2.0],
        [f32::NAN, 0.0, 2.0, 2.0],
    ] {
        let (_d, mut a) = open(&with_texture(vec![(1, own("s", rect, RECT))]));
        assert!(
            matches!(export_one(&mut a, "s"), Err(Error::Invalid(_))),
            "{rect:?}"
        );
    }
}

/// Lower-left triangle of the 4x4 sprite: pixel centres with x + y <= 3 (bottom-up) are in.
const TRIANGLE: Mesh = Mesh {
    vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
    indices: &[0, 1, 2],
    ..Mesh::BASE
};

fn tight(big: bool, unity_6000_5: bool, mesh: &Mesh) -> Vec<u8> {
    let r = [0.0, 0.0, 4.0, 4.0];
    sprite(
        big,
        unity_6000_5,
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
    )
}

fn kept(img: &Image) -> Vec<u8> {
    // Stored indices of the pixels left opaque, in output (top-down) order.
    img.rgba
        .chunks(4)
        .filter(|p| p[3] != 0)
        .map(|p| p[0] / 4)
        .collect()
}

const LOWER_LEFT: [u8; 10] = [12, 8, 9, 4, 5, 6, 0, 1, 2, 3];

fn masked(mesh: &Mesh) -> Result<Image, Error> {
    let (_d, mut a) = open(&with_texture(vec![(1, tight(false, false, mesh))]));
    export_one(&mut a, "s")
}

#[test]
fn tight_sprites_are_masked_to_their_mesh() {
    let img = masked(&TRIANGLE).unwrap();
    assert_eq!(kept(&img), LOWER_LEFT);
    // Masked pixels are cleared entirely.
    assert!(img
        .rgba
        .chunks(4)
        .filter(|p| p[3] == 0)
        .all(|p| p == [0, 0, 0, 0]));
    // Winding does not matter.
    let clockwise = Mesh {
        indices: &[0, 2, 1],
        ..TRIANGLE
    };
    assert_eq!(kept(&masked(&clockwise).unwrap()), LOWER_LEFT);
}

#[test]
fn mesh_layouts_are_read_as_unity_writes_them() {
    for mesh in [
        Mesh {
            pos_stream: 1,
            ..TRIANGLE
        },
        Mesh {
            base_vertex: 5,
            ..TRIANGLE
        },
        Mesh {
            junk_lines: true,
            ..TRIANGLE
        },
    ] {
        assert_eq!(kept(&masked(&mesh).unwrap()), LOWER_LEFT);
    }
    // Positions that are not float32 are not read, so the tight sprite is refused: half
    // floats, and 32-bit integers that would otherwise read as floats of the same size.
    for pos_format in [1, 10] {
        let other = Mesh {
            pos_format,
            ..TRIANGLE
        };
        assert!(
            matches!(masked(&other), Err(Error::Unsupported(_))),
            "{pos_format}"
        );
    }
}

#[test]
fn broken_meshes_are_errors_not_wrong_images() {
    // A degenerate point and a diagonal line alongside a real triangle change nothing.
    let with_junk = Mesh {
        vertices: &[
            [0.0, 0.0],
            [4.0, 0.0],
            [0.0, 4.0],
            [1.0, 1.0],
            [4.0, 4.0],
            [2.0, 2.0],
        ],
        indices: &[0, 1, 2, 3, 3, 3, 0, 4, 5],
        ..Mesh::BASE
    };
    assert_eq!(kept(&masked(&with_junk).unwrap()), LOWER_LEFT);
    // No triangle with any area: nothing to mask to.
    let flat = Mesh {
        indices: &[3, 3, 3, 0, 4, 5],
        ..with_junk
    };
    assert!(matches!(masked(&flat), Err(Error::Invalid(_))));
    // A corner that is not a number.
    let nan = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [f32::NAN, 4.0]],
        ..TRIANGLE
    };
    assert!(matches!(masked(&nan), Err(Error::Invalid(_))));
    // An index past the vertices makes the mesh unreadable, so the tight sprite is refused.
    let past = Mesh {
        indices: &[0, 1, 7],
        ..TRIANGLE
    };
    assert!(matches!(masked(&past), Err(Error::Unsupported(_))));
}

#[test]
fn big_endian_sprites_and_meshes() {
    let dir = TempDir::new("be");
    let file = serialized(
        21,
        "2019.4.40f1",
        true,
        19,
        &[
            (TEX, TEXTURE_2D, texture_object(true)),
            (1, SPRITE, tight(true, false, &TRIANGLE)),
        ],
    );
    let mut a = Assets::open(dir.file("s.assets", &file)).unwrap();
    assert_eq!(kept(&export_one(&mut a, "s").unwrap()), LOWER_LEFT);
}

#[test]
fn releases_after_6000_4_are_refused() {
    let tex = texture(
        Layout::U6000,
        false,
        "tex",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let objects = [
        (TEX, TEXTURE_2D, tex.clone()),
        (1, SPRITE, from_2023(tight(false, false, &TRIANGLE))),
    ];
    let (_d, a) = open_with(&objects, "6000.4.2f1", Limits::default());
    assert!(a.sprites(|_| true).skipped.is_empty());
    let objects = [
        (TEX, TEXTURE_2D, tex),
        (1, SPRITE, tight(false, true, &TRIANGLE)),
    ];
    let (_d, a) = open_with(&objects, "6000.5.0f1", Limits::default());
    assert!(matches!(
        a.sprites(|_| true).skipped[0].error,
        Error::Unsupported(_)
    ));
}

fn atlased(name: &str, key: i64) -> Vec<u8> {
    let r = [0.0, 0.0, 2.0, 2.0];
    sprite(
        false,
        false,
        name,
        r,
        [0.0, 0.0],
        key,
        ATLAS,
        0,
        0,
        r,
        RECT,
        1.0,
        &NO_MESH,
    )
}

#[test]
fn atlases_place_their_sprites() {
    let atlas_obj = atlas(
        false,
        &[
            (5, TEX, [1.0, 1.0, 2.0, 2.0], RECT, 1.0),
            (6, TEX, [0.0, 2.0, 2.0, 2.0], RECT, 0.5),
            (7, TEX, [0.0, 2.0, 2.0, 2.0], RECT, f32::NAN),
        ],
    );
    let r = [0.0, 0.0, 2.0, 2.0];
    // Not in the atlas's map, but with a texture of its own: falls back to that.
    let fallback = sprite(
        false,
        false,
        "fallback",
        r,
        [0.0, 0.0],
        9,
        ATLAS,
        TEX,
        0,
        r,
        RECT,
        1.0,
        &NO_MESH,
    );
    let (_d, mut a) = open(&[
        (TEX, TEXTURE_2D, texture_object(false)),
        (ATLAS, SPRITE_ATLAS, atlas_obj),
        (1, SPRITE, atlased("in", 5)),
        (2, SPRITE, atlased("missing", 9)),
        (3, SPRITE, atlased("downscaled", 6)),
        (4, SPRITE, atlased("nan", 7)),
        (8, SPRITE, fallback),
    ]);
    assert_eq!(ids(&export_one(&mut a, "in").unwrap()), [9, 10, 5, 6]);
    assert_eq!(ids(&export_one(&mut a, "fallback").unwrap()), [4, 5, 0, 1]);
    match export_one(&mut a, "missing") {
        Err(Error::Invalid(msg)) => assert!(msg.contains("not in its atlas"), "{msg}"),
        other => panic!("{other:?}"),
    }
    for name in ["downscaled", "nan"] {
        match export_one(&mut a, name) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains("scaled"), "{msg}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn atlases_before_2020_2_have_no_secondary_textures() {
    let entries = [
        (5, TEX, [1.0, 1.0, 2.0, 2.0], RECT, 1.0),
        (6, TEX, [0.0, 0.0, 2.0, 2.0], RECT, 1.0),
    ];
    let atlas_obj = atlas_with(false, false, &entries);
    let (_d, mut a) = open_with(
        &[
            (
                TEX,
                TEXTURE_2D,
                texture(
                    Layout::U2019_4,
                    false,
                    "tex",
                    4,
                    4,
                    format::RGBA32,
                    &Pixels::Inline(&rgba_4x4()),
                    &[],
                ),
            ),
            (ATLAS, SPRITE_ATLAS, atlas_obj),
            (1, SPRITE, atlased("first", 5)),
            (2, SPRITE, atlased("second", 6)),
        ],
        "2019.4.40f1",
        Limits::default(),
    );
    assert_eq!(ids(&export_one(&mut a, "first").unwrap()), [9, 10, 5, 6]);
    assert_eq!(ids(&export_one(&mut a, "second").unwrap()), [4, 5, 0, 1]);
}

#[test]
fn a_bad_atlas_spoils_only_its_own_sprites() {
    let mut broken = atlas(false, &[(5, TEX, [1.0, 1.0, 2.0, 2.0], RECT, 1.0)]);
    broken.truncate(30);
    let (_d, mut a) = open(&[
        (TEX, TEXTURE_2D, texture_object(false)),
        (ATLAS, SPRITE_ATLAS, broken),
        (1, SPRITE, atlased("in", 5)),
        (2, SPRITE, own("own", [1.0, 1.0, 2.0, 2.0], RECT)),
    ]);
    assert!(a.decode_texture(TEX).is_ok());
    assert!(export_one(&mut a, "own").is_ok());
    match export_one(&mut a, "in") {
        Err(Error::AtlasUnreadable { path_id, error, .. }) => {
            assert_eq!(path_id, ATLAS);
            assert!(matches!(*error, Error::Invalid(_)), "{error}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_bad_sprite_is_skipped_not_fatal() {
    let mut bad = own("bad", [1.0, 1.0, 2.0, 2.0], RECT);
    bad.truncate(40);
    let (_d, a) = open(&with_texture(vec![
        (1, own("good", [1.0, 1.0, 2.0, 2.0], RECT)),
        (2, bad),
    ]));
    let list = a.sprites(|_| true);
    assert_eq!(list.sprites.len(), 1);
    assert_eq!(list.skipped.len(), 1);
    assert_eq!(list.skipped[0].path_id, 2);
    assert_eq!(list.skipped[0].name.as_deref(), Some("bad"));
}

#[test]
fn sprites_come_ordered_by_texture() {
    let second = texture(
        Layout::U2022_3,
        false,
        "tex2",
        4,
        4,
        format::RGBA32,
        &Pixels::Inline(&rgba_4x4()),
        &[],
    );
    let r = [0.0, 0.0, 1.0, 1.0];
    let on = |name: &str, tex: i64| {
        sprite(
            false,
            false,
            name,
            r,
            [0.0, 0.0],
            1,
            0,
            tex,
            0,
            r,
            RECT,
            1.0,
            &NO_MESH,
        )
    };
    let (_d, a) = open(&[
        (TEX, TEXTURE_2D, texture_object(false)),
        (5, TEXTURE_2D, second),
        (1, SPRITE, on("a10", TEX)),
        (2, SPRITE, on("b5", 5)),
        (3, SPRITE, on("c10", TEX)),
    ]);
    let names: Vec<_> = a
        .sprites(|_| true)
        .sprites
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["b5", "a10", "c10"]);
}

#[test]
fn separate_alpha_textures_and_foreign_textures_are_refused() {
    let r = [1.0, 1.0, 2.0, 2.0];
    let alpha = sprite(
        false,
        false,
        "alpha",
        r,
        [0.0, 0.0],
        1,
        0,
        TEX,
        11,
        r,
        RECT,
        1.0,
        &NO_MESH,
    );
    let (_d, mut a) = open(&with_texture(vec![(1, alpha)]));
    match export_one(&mut a, "alpha") {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("alpha"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // A texture pointer into another file (file_id 1).
    let mut foreign = own("foreign", r, RECT);
    let tex_ptr = [0, 0, 0, 0, TEX as u8, 0, 0, 0, 0, 0, 0, 0];
    let at = foreign.windows(12).rposition(|w| w == tex_ptr).unwrap();
    foreign[at] = 1;
    let (_d, mut a) = open(&with_texture(vec![(1, foreign)]));
    match export_one(&mut a, "foreign") {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("another file"), "{msg}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cut_checks_what_it_is_given() {
    let (_d, a) = open(&with_texture(vec![(
        1,
        own("s", [1.0, 1.0, 2.0, 2.0], RECT),
    )]));
    let sprite = a.sprites(|_| true).sprites.remove(0);
    let small = Image::new(1, 1, vec![0; 4]).unwrap();
    assert!(matches!(a.cut(&sprite, &small), Err(Error::Invalid(_))));
    let mut lying = a.decode_texture(TEX).unwrap();
    lying.width += 1;
    assert!(a.cut(&sprite, &lying).is_err());
    lying.width -= 1;
    lying.rgba.truncate(10);
    assert!(a.cut(&sprite, &lying).is_err());
}

#[test]
fn limits_bound_meshes_masks_and_total_work() {
    let many = Mesh {
        indices: &[0, 1, 2, 0, 1, 2, 0, 1, 2],
        ..TRIANGLE
    };
    let limits = Limits::DEFAULT.with_max_sprite_triangles(2);
    let (_d, a) = open_with(
        &with_texture(vec![(1, tight(false, false, &many))]),
        "2022.3.62f1",
        limits,
    );
    let list = a.sprites(|_| true);
    assert!(matches!(
        list.skipped[0].error,
        Error::LimitExceeded {
            kind: LimitKind::SpriteTriangles,
            ..
        }
    ));

    // The list's total: the second sprite is refused before its mesh is built, and the
    // first still counts once.
    let limits = Limits::DEFAULT.with_max_total_triangles(1);
    let two = with_texture(vec![
        (1, tight(false, false, &TRIANGLE)),
        (2, tight(false, false, &TRIANGLE)),
    ]);
    let (_d, a) = open_with(&two, "2022.3.62f1", limits);
    let list = a.sprites(|_| true);
    assert_eq!((list.sprites.len(), list.skipped.len()), (1, 1));
    assert!(matches!(
        list.skipped[0].error,
        Error::LimitExceeded {
            kind: LimitKind::TotalTriangles,
            ..
        }
    ));
    let limits = Limits::DEFAULT.with_max_total_triangles(2);
    let (_d, a) = open_with(&two, "2022.3.62f1", limits);
    assert_eq!(a.sprites(|_| true).sprites.len(), 2);

    let limits = Limits::DEFAULT.with_max_mask_work(3);
    let (_d, mut a) = open_with(
        &with_texture(vec![(1, tight(false, false, &TRIANGLE))]),
        "2022.3.62f1",
        limits,
    );
    assert!(matches!(
        export_one(&mut a, "s"),
        Err(Error::LimitExceeded {
            kind: LimitKind::MaskWork,
            ..
        })
    ));

    // Decoding the 4x4 texture is 16 pixels of work, the cut 4 more.
    let one = with_texture(vec![(1, own("s", [1.0, 1.0, 2.0, 2.0], RECT))]);
    let limits = Limits::DEFAULT.with_max_total_work(19);
    let (_d, mut a) = open_with(&one, "2022.3.62f1", limits);
    assert!(matches!(
        export_one(&mut a, "s"),
        Err(Error::LimitExceeded {
            kind: LimitKind::TotalWork,
            ..
        })
    ));
    let limits = Limits::DEFAULT.with_max_total_work(20);
    let (_d, mut a) = open_with(&one, "2022.3.62f1", limits);
    assert!(export_one(&mut a, "s").is_ok());
}

#[test]
fn sub_mesh_ranges_must_fit_the_index_buffer() {
    let mut s = tight(false, false, &TRIANGLE);
    // The sub-mesh's index count sits after its first_byte; make it claim 1000 indices.
    let at = s
        .windows(8)
        .position(|w| w == [0, 0, 0, 0, 3, 0, 0, 0])
        .unwrap();
    s[at + 4..at + 8].copy_from_slice(&1000u32.to_le_bytes());
    let (_d, a) = open(&with_texture(vec![(1, s)]));
    match &a.sprites(|_| true).skipped[..] {
        [skip] => assert!(
            skip.error.to_string().contains("index buffer"),
            "{}",
            skip.error
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn old_engines_list_their_sprites_as_skipped() {
    let s = own("s", [1.0, 1.0, 2.0, 2.0], RECT);
    let (_d, a) = open_with(&[(1, SPRITE, s)], "2018.4.36f1", Limits::default());
    let list = a.sprites(|_| true);
    assert!(list.sprites.is_empty());
    assert!(matches!(list.skipped[0].error, Error::Unsupported(_)));
}

#[test]
fn errors_quote_names_from_the_file() {
    let r = [0.0, 0.0, 2.0, 2.0];
    let evil = "boom\x1b]0;pwned\x07\x1b[2J\n";
    let s = sprite(
        false,
        false,
        evil,
        r,
        [0.0, 0.0],
        1,
        0,
        0,
        0,
        r,
        RECT,
        1.0,
        &NO_MESH,
    );
    let (_d, mut a) = open(&with_texture(vec![(1, s)]));
    let err = export_one(&mut a, evil).unwrap_err().to_string();
    assert!(err.contains("boom"), "{err}");
    assert!(!err.chars().any(char::is_control), "{err:?}");
}
