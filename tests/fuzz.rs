//! A mutation fuzzer, std only: take well-formed files built by `common`, damage them in ways
//! that matter to a parser (truncation, flipped bytes, boundary values in 32-bit fields), and
//! run every public read over each. Any panic, or any input taking more than two seconds,
//! fails the test.
//!
//! `UBA_FUZZ_ITERS` sets the number of mutations per seed (default 1,500). Run longer with
//! `UBA_FUZZ_ITERS=200000 cargo test --release --test fuzz`.

mod common;
use common::*;
use std::sync::Arc;
use unity_bundle_assets::{Assets, Bundle, SerializedFile};

fn seeds() -> Vec<(bool, Vec<u8>)> {
    let tex = texture(
        Layout::U2022_3,
        false,
        "tex",
        4,
        4,
        format::DXT5,
        &Pixels::Inline(&[0x5a; 16]),
        &[],
    );
    let tri = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let r = [0.0, 0.0, 4.0, 4.0];
    let tight = sprite(
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
        0,
        1.0,
        &tri,
    );
    let atlased = sprite(
        false,
        false,
        "a",
        [0.0, 0.0, 2.0, 2.0],
        [0.0, 0.0],
        5,
        30,
        0,
        0,
        r,
        2,
        1.0,
        &Mesh::BASE,
    );
    let atl = atlas(
        false,
        &[(5, 10, [1.0, 1.0, 2.0, 2.0], 0b01 | 0b10 | 4 << 2, 1.0)],
    );
    let file = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &[
            (10, TEXTURE_2D, tex),
            (1, SPRITE, tight),
            (2, SPRITE, atlased),
            (30, SPRITE_ATLAS, atl),
        ],
    );
    let old = serialized(
        17,
        "5.6.6f2",
        false,
        19,
        &[(
            7,
            TEXTURE_2D,
            texture(
                Layout::U5_6,
                false,
                "t",
                4,
                4,
                format::RGB24,
                &Pixels::Inline(&[1; 48]),
                &[],
            ),
        )],
    );
    let mut seeds = vec![(false, file.clone()), (false, old.clone())];
    for (blocks, info, at_end) in [
        (vec![0], 0, false),
        (vec![2, 1], 0, true),
        (vec![3], 2, false),
    ] {
        let mut o = BundleOpts::new(6, "2022.3.62f1");
        o.blocks = blocks;
        o.info = info;
        o.info_at_end = at_end;
        seeds.push((
            true,
            bundle(&o, &[("CAB-a", &file, 4), ("CAB-a.resS", &[7; 64], 0)]),
        ));
    }
    seeds
}

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// How far mutated inputs got: files opened, textures decoded, sprites exported.
static OPENED: AtomicUsize = AtomicUsize::new(0);
static DECODED: AtomicUsize = AtomicUsize::new(0);
static EXPORTED: AtomicUsize = AtomicUsize::new(0);

fn exercise_assets(mut a: Assets) {
    OPENED.fetch_add(1, Relaxed);
    for t in a.textures(|_| true) {
        if a.decode_texture(t.path_id).is_ok() {
            DECODED.fetch_add(1, Relaxed);
        }
    }
    let list = a.sprites(|_| true);
    for s in &list.sprites {
        if a.export(s).is_ok() {
            EXPORTED.fetch_add(1, Relaxed);
        }
    }
}

fn exercise(is_bundle: bool, data: &[u8]) {
    if is_bundle {
        if let Ok(b) = Bundle::parse(data) {
            let b = Arc::new(b);
            let names: Vec<String> = b.serialized_files().map(|e| e.path().to_string()).collect();
            for n in names {
                if let Ok(a) = Assets::from_bundle(b.clone(), &n) {
                    exercise_assets(a);
                }
            }
        }
    } else if let Ok(f) = SerializedFile::parse(data.to_vec()) {
        if let Ok(a) = Assets::from_serialized(f, std::env::temp_dir()) {
            exercise_assets(a);
        }
    }
}

#[test]
fn mutated_files_never_panic_or_hang() {
    let iters: usize = std::env::var("UBA_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    for (is_bundle, original) in seeds() {
        // Control: the unmutated seed must parse.
        assert!(
            if is_bundle {
                Bundle::parse(&original).is_ok()
            } else {
                SerializedFile::parse(original.clone()).is_ok()
            },
            "seed does not parse"
        );
        for _ in 0..iters {
            let mut m = original.clone();
            match rnd() % 4 {
                0 => m.truncate(rnd() as usize % m.len()),
                1 => {
                    for _ in 0..1 + rnd() % 8 {
                        let k = rnd() as usize % m.len();
                        m[k] = rnd() as u8;
                    }
                }
                2 => {
                    for _ in 0..1 + rnd() % 4 {
                        let k = rnd() as usize % m.len();
                        m[k] ^= 1 << (rnd() % 8);
                    }
                }
                _ => {
                    let k = (rnd() as usize % m.len()) & !3;
                    let v = [0u32, 1, 0x7fff_ffff, 0xffff_ffff, 0x8000_0000, 0xffff]
                        [(rnd() % 6) as usize];
                    if k + 4 <= m.len() {
                        let bytes = if rnd() % 2 == 0 {
                            v.to_le_bytes()
                        } else {
                            v.to_be_bytes()
                        };
                        m[k..k + 4].copy_from_slice(&bytes);
                    }
                }
            }
            let start = std::time::Instant::now();
            let panicked = std::panic::catch_unwind(|| exercise(is_bundle, &m)).is_err();
            let slow = start.elapsed().as_secs_f32() > 2.0;
            if panicked || slow {
                failures.push((is_bundle, panicked, slow, m));
            }
        }
    }
    let _ = std::panic::take_hook();
    let (opened, decoded, exported) = (
        OPENED.load(Relaxed),
        DECODED.load(Relaxed),
        EXPORTED.load(Relaxed),
    );
    eprintln!("{iters} mutations per seed: {opened} opened, {decoded} textures decoded, {exported} sprites exported");
    // Control: mutations must leave enough intact to reach the decoders, or this tests only
    // the first few header checks.
    assert!(
        opened > iters && decoded > 0 && exported > 0,
        "mutations never reach the decoders"
    );
    assert!(
        failures.is_empty(),
        "{} inputs failed; first: bundle {} panicked {} slow {}",
        failures.len(),
        failures[0].0,
        failures[0].1,
        failures[0].2
    );
}
