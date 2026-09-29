//! The example programs as users run them: the real binaries, with their `main`, exit codes,
//! standard output and error, and a closed pipe. (Each example's own tests call its `run`
//! directly and check what it prints.)

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// The built example binary `name`, building all the examples once per test run. They land
/// beside this test's own binary: `<target>/<profile>/examples/`.
fn example(name: &str) -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let dir = BUILT.get_or_init(|| {
        let profile_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        let mut cargo = Command::new(env!("CARGO"));
        cargo
            .args(["build", "--examples", "--quiet"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cargo.arg("--release");
        }
        let status = cargo.status().unwrap();
        assert!(status.success(), "building the examples failed");
        profile_dir.join("examples")
    });
    let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(path.exists(), "no example binary at {}", path.display());
    path
}

/// A Unity 2022.3 file: a 4x2 texture "tex" (10), a sprite "icon" on it (1), and with `bad`
/// a texture "astc" (20) in a format the crate does not decode.
fn sample(dir: &TempDir, bad: bool) -> PathBuf {
    let rgba: Vec<u8> = (0..32).collect();
    let tex = texture(
        Layout::U2022_3,
        false,
        "tex",
        4,
        2,
        format::RGBA32,
        &Pixels::Inline(&rgba),
        &[],
    );
    let r = [0.0, 0.0, 2.0, 1.0];
    let icon = sprite(
        false,
        false,
        "icon",
        r,
        [0.0, 0.0],
        1,
        0,
        10,
        0,
        r,
        0b10,
        1.0,
        &Mesh::BASE,
    );
    let mut objects = vec![(10, TEXTURE_2D, tex), (1, SPRITE, icon)];
    if bad {
        let astc = texture(
            Layout::U2022_3,
            false,
            "astc",
            4,
            4,
            format::ASTC_4X4,
            &Pixels::Inline(&[0; 16]),
            &[],
        );
        objects.push((20, TEXTURE_2D, astc));
    }
    dir.file(
        "sample.assets",
        &serialized(22, "2022.3.62f1", false, 19, &objects),
    )
}

fn run(name: &str, args: &[&std::ffi::OsStr]) -> Output {
    Command::new(example(name)).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn a_good_run_prints_to_stdout_and_exits_0() {
    let dir = TempDir::new("cli-ok");
    let file = sample(&dir, false);
    let out = run("list", &[file.as_os_str()]);
    assert!(out.status.success(), "{out:?}");
    assert!(
        text(&out.stdout).starts_with("format 22 unity 2022.3.62f1"),
        "{out:?}"
    );
    assert!(out.stderr.is_empty(), "{out:?}");

    let pngs = dir.0.join("pngs");
    let out = run("export", &[file.as_os_str(), pngs.as_os_str()]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "exported 1, failed 0\n");
    assert!(pngs.join("icon.png").exists());
}

#[test]
fn a_failed_run_says_why_on_stderr_and_exits_1() {
    let dir = TempDir::new("cli-fail");
    // No arguments: the usage.
    let out = run("dump", &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        text(&out.stderr).starts_with("error: usage: dump "),
        "{out:?}"
    );
    assert!(out.stdout.is_empty());
    // A missing file: named, nothing made.
    let missing = dir.0.join("missing.assets");
    let pngs = dir.0.join("pngs");
    let out = run("textures", &[missing.as_os_str(), pngs.as_os_str()]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(text(&out.stderr).starts_with("error: "), "{out:?}");
    assert!(!pngs.exists());
    // One texture it cannot decode: the rest exported, the failure named, exit 1.
    let file = sample(&dir, true);
    let out = run("textures", &[file.as_os_str(), pngs.as_os_str()]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(text(&out.stdout), "exported 1, failed 1\n");
    let err = text(&out.stderr);
    assert!(err.starts_with("astc (20): "), "{err}");
    assert!(err.ends_with("error: 1 not exported\n"), "{err}");
    assert!(pngs.join("tex_10.png").exists());
}

#[test]
fn a_closed_stdout_is_not_a_failure_but_a_failed_run_still_is() {
    let dir = TempDir::new("cli-pipe");
    let closed = |name: &str, args: &[&std::ffi::OsStr]| {
        let mut child = Command::new(example(name))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        drop(child.stdout.take()); // the reader goes away, as `| head -0` does
        child.wait().unwrap()
    };
    let good = sample(&dir, false);
    assert!(closed("list", &[good.as_os_str()]).success());
    let bad = sample(&dir, true);
    let pngs = dir.0.join("pngs");
    assert_eq!(
        closed("textures", &[bad.as_os_str(), pngs.as_os_str()]).code(),
        Some(1)
    );
}
