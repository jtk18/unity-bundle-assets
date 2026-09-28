//! A mutation fuzzer, std only: take well-formed files built by `common`, damage them in ways
//! that matter to a parser (truncation, flipped bytes, boundary values in 32-bit fields), and
//! run every public read over each. Any panic, or any input taking more than two seconds,
//! fails the test; an input still running after twenty is saved and the process stopped.
//! Failing inputs are saved under `target/tmp/fuzz-failures`.
//!
//! `UBA_FUZZ_ITERS` sets the number of mutations per seed (default 1,500) and
//! `UBA_FUZZ_SEED` the generator's seed (printed at the start of each run). Run longer with
//! `UBA_FUZZ_ITERS=200000 cargo test --release --test fuzz -- --nocapture`.

mod common;
use common::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use unity_bundle_assets::{Assets, Bundle, SerializedFile};

/// What a seed is, and so which reads to run over it.
enum Kind {
    Serialized,
    /// A serialized file whose textures stream from this folder.
    Streamed(std::path::PathBuf),
    Bundle,
}

struct Seed {
    name: &'static str,
    kind: Kind,
    data: Vec<u8>,
}

fn seeds(stream_dir: &TempDir) -> Vec<Seed> {
    let tri = Mesh {
        vertices: &[[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]],
        indices: &[0, 1, 2],
        ..Mesh::BASE
    };
    let r = [0.0, 0.0, 4.0, 4.0];
    let objects = |big: bool, px: &Pixels| {
        let tex = texture(Layout::U2022_3, big, "tex", 4, 4, format::DXT5, px, &[]);
        let tight = sprite(big, false, "s", r, [0.0, 0.0], 1, 0, 10, 0, r, 0, 1.0, &tri);
        let atlased = sprite(
            big,
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
            big,
            &[(5, 10, [1.0, 1.0, 2.0, 2.0], 0b01 | 0b10 | 4 << 2, 1.0)],
        );
        vec![
            (10, TEXTURE_2D, tex),
            (1, SPRITE, tight),
            (2, SPRITE, atlased),
            (30, SPRITE_ATLAS, atl),
        ]
    };
    let inline = Pixels::Inline(&[0x5a; 16]);
    let file = serialized(22, "2022.3.62f1", false, 19, &objects(false, &inline));
    let big = serialized(22, "2022.3.62f1", true, 19, &objects(true, &inline));
    let extras = Extras {
        type_trees: true,
        mono: true,
        scripts: 2,
        externals: vec!["library/unity default resources".into()],
    };
    let with_extras = serialized_with(
        22,
        "2022.3.62f1",
        false,
        19,
        &objects(false, &inline),
        &extras,
    );
    stream_dir.file("s.resS", &[0x3c; 64]);
    let streamed = serialized(
        22,
        "2022.3.62f1",
        false,
        19,
        &objects(
            false,
            &Pixels::Streamed {
                path: "s.resS",
                offset: 16,
                size: 16,
            },
        ),
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
    let plain = |name, data| Seed {
        name,
        kind: Kind::Serialized,
        data,
    };
    let mut seeds = vec![
        plain("serialized", file.clone()),
        plain("big-endian", big),
        plain("extras", with_extras),
        plain("5.6", old),
        Seed {
            name: "streamed",
            kind: Kind::Streamed(stream_dir.0.clone()),
            data: streamed,
        },
    ];
    for (name, format, revision, blocks, info, at_end) in [
        ("bundle-stored", 6, "2022.3.62f1", vec![0], 0, false),
        ("bundle-lz4hc-lzma", 6, "2022.3.62f1", vec![2, 1], 0, true),
        ("bundle-lz4-matches", 6, "2022.3.62f1", vec![4], 2, false),
        ("bundle-format-7", 7, "2019.4.40f1", vec![4, 0], 2, false),
        ("bundle-format-8", 8, "2022.3.62f1", vec![4], 2, true),
    ] {
        let mut o = BundleOpts::new(format, revision);
        o.blocks = blocks;
        o.info = info;
        o.info_at_end = at_end;
        o.padding_flag = format == 8;
        seeds.push(Seed {
            name,
            kind: Kind::Bundle,
            data: bundle(&o, &[("CAB-a", &file, 4), ("CAB-a.resS", &[7; 64], 0)]),
        });
    }
    seeds
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

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

fn exercise(kind: &Kind, data: &[u8]) {
    match kind {
        Kind::Bundle => {
            if let Ok(b) = Bundle::parse(data) {
                let b = Arc::new(b);
                let names: Vec<String> =
                    b.serialized_files().map(|e| e.path().to_string()).collect();
                for n in names {
                    if let Ok(a) = Assets::from_bundle(b.clone(), &n) {
                        exercise_assets(a);
                    }
                }
            }
        }
        Kind::Serialized | Kind::Streamed(_) => {
            if let Ok(f) = SerializedFile::parse(data.to_vec()) {
                // A plain file gets an empty folder, so a mutated stream name finds nothing.
                let dir = match kind {
                    Kind::Streamed(dir) => dir.clone(),
                    _ => EMPTY.with(PathBuf::clone),
                };
                if let Ok(a) = Assets::from_serialized(f, dir) {
                    exercise_assets(a);
                }
            }
        }
    }
}

thread_local! {
    static EMPTY: PathBuf = TempDir::new("fuzz-empty").0.clone();
}

fn failures_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fuzz-failures");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The input being run, for the watchdog.
struct Running {
    since: Option<Instant>,
    name: String,
    input: Vec<u8>,
}

/// Stop the process if one input runs past `limit`, saving it first: a hang would otherwise
/// hold the test forever and lose the input.
fn watchdog(running: Arc<Mutex<Running>>, done: Arc<AtomicBool>, limit: Duration) {
    std::thread::spawn(move || {
        while !done.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
            let hung = {
                let r = running
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                r.since
                    .is_some_and(|t| t.elapsed() > limit)
                    .then(|| (r.name.clone(), r.input.clone()))
            };
            if let Some((name, input)) = hung {
                let path = failures_dir().join(format!("{name}-hang.bin"));
                let _ = std::fs::write(&path, &input);
                // Straight to stderr: the test harness's capture is lost on exit.
                let _ = writeln!(
                    std::io::stderr(),
                    "input hung for {limit:?}; saved to {}",
                    path.display()
                );
                std::process::exit(101);
            }
        }
    });
}

fn mutate(original: &[u8], rnd: &mut impl FnMut() -> u64) -> Vec<u8> {
    let mut m = original.to_vec();
    match rnd() % 4 {
        0 => m.truncate(rnd() as usize % m.len()),
        1 => {
            for _ in 0..=(rnd() % 8) {
                let k = rnd() as usize % m.len();
                m[k] = rnd() as u8;
            }
        }
        2 => {
            for _ in 0..=(rnd() % 4) {
                let k = rnd() as usize % m.len();
                m[k] ^= 1 << (rnd() % 8);
            }
        }
        _ => {
            let k = (rnd() as usize % m.len()) & !3;
            let v = [0u32, 1, 0x7fff_ffff, 0xffff_ffff, 0x8000_0000, 0xffff][(rnd() % 6) as usize];
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
    m
}

#[test]
fn mutated_files_never_panic_or_hang() {
    let iters: usize = std::env::var("UBA_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1500);
    let mut seed: u64 = std::env::var("UBA_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    eprintln!("UBA_FUZZ_SEED={seed}");
    assert_ne!(seed, 0, "xorshift needs a non-zero seed");
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let stream_dir = TempDir::new("fuzz-streams");
    let running = Arc::new(Mutex::new(Running {
        since: None,
        name: String::new(),
        input: Vec::new(),
    }));
    let done = Arc::new(AtomicBool::new(false));
    watchdog(running.clone(), done.clone(), Duration::from_secs(20));
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    let mut unopened = Vec::new();
    for seed in seeds(&stream_dir) {
        // Control: the unmutated seed must open and reach the decoders.
        let before = (OPENED.load(Relaxed), DECODED.load(Relaxed));
        exercise(&seed.kind, &seed.data);
        let after = (OPENED.load(Relaxed), DECODED.load(Relaxed));
        assert!(
            after.0 > before.0 && after.1 > before.1,
            "seed {} does not open and decode",
            seed.name
        );
        let opened_before = OPENED.load(Relaxed);
        for i in 0..iters {
            let m = mutate(&seed.data, &mut rnd);
            {
                let mut r = running.lock().unwrap();
                r.name = format!("{}-{i}", seed.name);
                r.input.clone_from(&m);
                r.since = Some(Instant::now());
            }
            let start = Instant::now();
            let panicked = std::panic::catch_unwind(|| exercise(&seed.kind, &m)).is_err();
            let slow = start.elapsed() > Duration::from_secs(2);
            running.lock().unwrap().since = None;
            if panicked || slow {
                let path = failures_dir().join(format!("{}-{i}.bin", seed.name));
                std::fs::write(&path, &m).unwrap();
                failures.push(format!(
                    "{} (panicked {panicked}, slow {slow})",
                    path.display()
                ));
            }
        }
        // Control, per seed: mutations must leave enough intact to open the file, or this
        // seed tests only the first few header checks. A wholly compressed bundle opens least
        // (about 7% of mutations at the default count), since most damage breaks its blocks.
        let opened = OPENED.load(Relaxed) - opened_before;
        eprintln!("{}: {opened} of {iters} mutations opened", seed.name);
        if opened < iters / 20 {
            unopened.push(seed.name);
        }
    }
    done.store(true, Relaxed);
    std::panic::set_hook(previous_hook);
    let (opened, decoded, exported) = (
        OPENED.load(Relaxed),
        DECODED.load(Relaxed),
        EXPORTED.load(Relaxed),
    );
    eprintln!("{iters} mutations per seed: {opened} opened, {decoded} textures decoded, {exported} sprites exported");
    assert!(unopened.is_empty(), "mutations of {unopened:?} rarely open");
    assert!(
        decoded > iters && exported > iters,
        "mutations rarely reach the decoders"
    );
    assert!(
        failures.is_empty(),
        "{} inputs failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
