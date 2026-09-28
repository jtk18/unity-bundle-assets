//! Every count in a file is checked against the bytes after it at its record's real minimum
//! size, before anything is allocated for it. The sites are found, not listed: each place
//! where a huge count is refused *at that place* is one. For each, the largest count accepted
//! is found with and without `PAD * size` more bytes after it; the two must differ by exactly
//! `PAD`, which pins the minimum size to the byte.

mod common;
use common::*;
use unity_bundle_assets::{Bundle, Error, SerializedFile, Sprite, SpriteAtlas};

const HUGE: i32 = 0x7fff_ffff;
const PAD: usize = 1000;

/// Parses a file and reports a bad length's `(at, len)`, if any.
type Check<'a> = &'a dyn Fn(&[u8]) -> Option<(usize, i64)>;

/// The `(at, len)` of a bad length in `result`: the error itself, or quoted in a layout
/// error's message.
fn bad_length<T>(result: Result<T, Error>) -> Option<(usize, i64)> {
    match result {
        Err(Error::BadLength { at, len, .. }) => Some((at, len)),
        Err(e) => {
            let msg = e.to_string();
            let rest = &msg[msg.find("bad length ")? + 11..];
            let (len, rest) = rest.split_once(" at byte ")?;
            let at: String = rest.chars().take_while(char::is_ascii_digit).collect();
            Some((at.parse().ok()?, len.parse().ok()?))
        }
        Ok(_) => None,
    }
}

fn write(data: &mut [u8], p: usize, n: i32, big: bool) {
    data[p..p + 4].copy_from_slice(&if big {
        n.to_be_bytes()
    } else {
        n.to_le_bytes()
    });
}

/// The count sites in `data`: `(position in data, offset the error reports)`.
fn sites(data: &[u8], big: bool, check: Check<'_>) -> Vec<(usize, usize)> {
    (0..=data.len() - 4)
        .filter_map(|p| {
            let mut d = data.to_vec();
            write(&mut d, p, HUGE, big);
            match check(&d) {
                Some((at, len)) if len == i64::from(HUGE) => Some((p, at)),
                _ => None,
            }
        })
        .collect()
}

/// The largest count the site at `p` accepts (another error later is fine).
fn largest_accepted(data: &[u8], p: usize, at: usize, big: bool, check: Check<'_>) -> i64 {
    let refused = |n: i32| {
        let mut d = data.to_vec();
        write(&mut d, p, n, big);
        check(&d).is_some_and(|(a, _)| a == at)
    };
    let (mut lo, mut hi) = (0i32, HUGE);
    assert!(!refused(0) && refused(HUGE));
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if refused(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    i64::from(lo)
}

/// The minimum record size each count site uses, in order: `data` and `padded` differ only
/// in `PAD * size` bytes somewhere after every site, for each candidate size.
fn measured_sizes(
    build: &dyn Fn(usize) -> Vec<u8>,
    big: bool,
    check: Check<'_>,
    sizes: &[usize],
) -> Vec<(usize, i64)> {
    let data = build(0);
    let found = sites(&data, big, check);
    assert_eq!(found.len(), sizes.len(), "sites {found:?}");
    found
        .iter()
        .zip(sizes)
        .map(|(&(p, at), &size)| {
            let padded = build(PAD * size);
            let grew = largest_accepted(&padded, p, at, big, check)
                - largest_accepted(&data, p, at, big, check);
            (p, grew)
        })
        .collect()
}

fn assert_sizes(what: &str, got: &[(usize, i64)]) {
    for &(p, grew) in got {
        assert_eq!(grew, PAD as i64, "{what}: the count at byte {p}");
    }
}

#[test]
fn serialized_file_counts() {
    for (version, unity) in [
        (17, "2018.4.36f1"),
        (18, "2018.4.36f1"),
        (19, "2019.4.40f1"),
        (20, "2019.4.40f1"),
        (21, "2020.3.48f1"),
        (22, "2022.3.62f1"),
    ] {
        let extras = Extras {
            type_trees: true,
            mono: true,
            scripts: 1,
            externals: vec!["x".into()],
        };
        // A last object whose size is the padding, after every count.
        let build = |pad: usize| {
            serialized_with(
                version,
                unity,
                false,
                19,
                &[(1, 1, vec![0; 8 + pad])],
                &extras,
            )
        };
        let check = |d: &[u8]| bad_length(SerializedFile::parse(d.to_vec()));
        // Version string, then: types; per type, type-tree nodes and strings, and (v21 on)
        // dependencies; objects; scripts; externals, each with its empty path.
        let deps = if version >= 21 { vec![4] } else { vec![] };
        let node = if version >= 19 { 32 } else { 24 };
        let mut sizes = vec![23];
        for _ in 0..2 {
            sizes.extend([node, 1]);
            sizes.extend(&deps);
        }
        sizes.extend([if version >= 22 { 24 } else { 20 }, 12, 22]);
        let got = measured_sizes(&build, false, &check, &sizes);
        assert_sizes(&format!("v{version}"), &got);
    }
}

#[test]
fn object_and_bundle_counts() {
    let r = [0.0, 0.0, 1.0, 1.0];
    let outline: &[[f32; 2]] = &[[0.0, 0.0], [1.0, 0.0]];
    let extras = SpriteExtras {
        atlas_tags: &["tag"],
        secondary_textures: &[(11, "_Normal")],
        bindposes: 0,
        outlines: &[outline],
    };
    // Each object last in its file, padded at its end.
    let in_file = |class: i32, mut object: Vec<u8>, pad: usize, unity: &str| {
        object.extend(vec![0; pad]);
        serialized(22, unity, false, 19, &[(1, class, object)])
    };
    for (unity, bones, scriptables) in [
        ("2020.3.48f1", 40, false),
        ("2022.3.62f1", 48, false),
        ("2023.2.20f1", 48, true),
    ] {
        let sprite_bytes = || {
            let s = sprite_ext(
                false,
                false,
                "s",
                r,
                [0.0, 0.0],
                1,
                0,
                10,
                0,
                r,
                2,
                1.0,
                &Mesh::BASE,
                1.0,
                [0.0, 0.0],
                &extras,
            );
            let s = with_bone(s, bones == 48);
            if scriptables {
                from_2023(s)
            } else {
                s
            }
        };
        let build = |pad: usize| in_file(SPRITE, sprite_bytes(), pad, unity);
        let check = |d: &[u8]| {
            let file = SerializedFile::parse(d.to_vec()).ok()?;
            bad_length(Sprite::read(&file, &file.objects()[0]))
        };
        // Name; atlas tags and one tag; secondary textures and one name; sub-meshes; index
        // buffer; channels; vertex data; bindposes; outlines and one outline's points; bones,
        // one bone's name and (from 2021.1) GUID; (from 2023.1) scriptable objects.
        let mut sizes = vec![1, 4, 1, 16, 1, 48, 1, 4, 1, 64, 4, 8, bones, 1];
        if bones == 48 {
            sizes.push(1);
        }
        if scriptables {
            sizes.push(12);
        }
        assert_sizes(
            &format!("sprite {unity}"),
            &measured_sizes(&build, false, &check, &sizes),
        );
    }

    // Name, limit group name, platform blob, pixels, stream path.
    let build = |pad: usize| {
        let t = texture(
            Layout::U2022_3,
            false,
            "tex",
            4,
            4,
            format::RGBA32,
            &Pixels::Inline(&[1; 64]),
            &[5; 3],
        );
        in_file(TEXTURE_2D, t, pad, "2022.3.62f1")
    };
    let check = |d: &[u8]| {
        let file = SerializedFile::parse(d.to_vec()).ok()?;
        bad_length(unity_bundle_assets::Texture2D::read(&file, &file.objects()[0]).map(|_| ()))
    };
    assert_sizes(
        "texture",
        &measured_sizes(&build, false, &check, &[1, 1, 1, 1, 1]),
    );

    // Name, packed sprites, their names and one name, entries (with, from 2020.2, their
    // secondary textures), tag.
    for (unity, secondary, entry) in [("2020.1.17f1", false, 104), ("2022.3.62f1", true, 108)] {
        let build = |pad: usize| {
            let a = atlas_full(
                false,
                secondary,
                "atlas",
                &[(1, "s")],
                &[(5, 10, r, 2, 1.0)],
            );
            in_file(SPRITE_ATLAS, a, pad, unity)
        };
        let check = |d: &[u8]| {
            let file = SerializedFile::parse(d.to_vec()).ok()?;
            bad_length(SpriteAtlas::read(&file, &file.objects()[0]))
        };
        let mut sizes = vec![1, 12, 4, 1, entry];
        if secondary {
            sizes.push(16);
        }
        sizes.push(1);
        assert_sizes(
            &format!("atlas {unity}"),
            &measured_sizes(&build, false, &check, &sizes),
        );
    }

    // The directory: blocks, entries.
    let build = |pad: usize| {
        let mut o = BundleOpts::new(6, "2018.4.36f1");
        o.info_padding = pad;
        bundle(&o, &[("a.resS", &[1; 8], 0)])
    };
    let check = |d: &[u8]| bad_length(Bundle::parse(d));
    assert_sizes("bundle", &measured_sizes(&build, true, &check, &[10, 21]));
}
