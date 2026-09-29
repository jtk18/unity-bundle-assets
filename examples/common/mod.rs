//! Helpers the examples share. Names come from the file being read, so they are cleaned
//! before they touch a path or a terminal.

#![allow(dead_code, reason = "each example uses only some of these helpers")]

use std::collections::HashSet;

/// A name reduced to one safe path component: letters, digits, `.`, `_` and `-`, at most 100
/// bytes, never empty, never `.` or `..`, never starting with `-` (which a later shell command
/// would read as an option), and never a Windows device name.
pub fn file_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if safe.is_empty() || safe.chars().all(|c| c == '.') || safe.starts_with('-') || device {
        safe.insert(0, '_');
        safe.truncate(100);
    }
    safe
}

/// The next argument as text, or an error naming it.
pub fn text_arg(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    what: &str,
) -> Result<Option<String>, String> {
    args.next()
        .map(|a| a.into_string().map_err(|_| format!("{what} is not UTF-8")))
        .transpose()
}

/// Output names already used in this run, compared as a case-insensitive file system would.
#[derive(Default)]
pub struct Names(HashSet<String>);

impl Names {
    /// `base`, or `base_<id>` (then `base_<id>_2`, ...) until the name is free, ignoring case.
    pub fn claim(&mut self, base: &str, id: i64) -> String {
        let mut name = base.to_string();
        let mut n = 1;
        while !self.0.insert(name.to_lowercase()) {
            name = if n == 1 {
                format!("{base}_{id}")
            } else {
                format!("{base}_{id}_{n}")
            };
            n += 1;
        }
        name
    }
}

/// A string safe to print to a terminal: printable ASCII as it is (a backslash doubled, so
/// escapes cannot be forged), everything else shown as `\u{...}`. Names from a file can hold
/// characters that are blank, reorder text or look like others; no list of the harmful ones
/// keeps up with Unicode, so none is trusted.
pub fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // A backslash too, so a name holding the text `\u{202e}` does not read as one
            // holding the character.
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            _ => out.extend(c.escape_unicode()),
        }
    }
    out
}

/// `s` cut to [`MAX_LINE`] characters: cut before escaping, so a long name costs no more than
/// a short one and the cut never falls inside an escape.
pub fn cut(s: &str) -> &str {
    s.char_indices().nth(MAX_LINE).map_or(s, |(at, _)| &s[..at])
}

/// An error message safe to print: the crate's own messages are already escaped (with `\`
/// escapes), so only what is not printable ASCII is escaped here, and backslashes are left as
/// they are; other errors (I/O, PNG) get the same treatment.
pub fn printable_error(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' '..='~' => out.push(c),
            _ => out.extend(c.escape_unicode()),
        }
    }
    out
}

/// Write a top-down RGBA image as PNG to a new file. Anything already at `path` is left alone
/// and reported: a file from an earlier run, or a link, FIFO or device someone placed there,
/// which writing through would overwrite something else or hang. Creating the file only if it
/// is new (`O_CREAT | O_EXCL`) refuses all of them in one step, with no gap between a check
/// and the write.
pub fn save_png(
    path: &std::path::Path,
    image: &unity_bundle_assets::Image,
) -> Result<(), Box<dyn std::error::Error>> {
    save_png_through(path, image, std::io::BufWriter::new)
}

/// [`save_png`], writing through `wrap` of the new file (under test, a writer that fails).
fn save_png_through<W: std::io::Write>(
    path: &std::path::Path,
    image: &unity_bundle_assets::Image,
    wrap: impl FnOnce(std::fs::File) -> W,
) -> Result<(), Box<dyn std::error::Error>> {
    use image::ImageEncoder;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // Flushed here, not on drop, so a disk that fills up at the end is an error, not a
    // silently truncated PNG.
    let mut out = wrap(file);
    let written = image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            image.rgba(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| e.to_string())
        .and_then(|()| std::io::Write::flush(&mut out).map_err(|e| e.to_string()));
    if let Err(e) = written {
        // The file is ours (made new above) and damaged: remove it, so a rerun can write it.
        drop(out);
        let _ = std::fs::remove_file(path);
        return Err(format!("{}: {e}", path.display()).into());
    }
    Ok(())
}

/// Print a line to standard output, returning the error instead of panicking when it cannot
/// be written (a closed pipe, as `| head` makes).
#[allow(unused_macros, reason = "each example uses only some of these helpers")]
macro_rules! outln {
    ($($arg:tt)*) => {
        common::write_line(format_args!($($arg)*))?
    };
}

/// [`outln!`]'s writer.
#[cfg(not(test))]
pub fn write_line(line: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    use std::io::Write;
    writeln!(std::io::stdout().lock(), "{line}")
}

/// Under test, through `println!`, which the test harness captures.
#[cfg(test)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the same signature as the real one"
)]
pub fn write_line(line: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    // Mutation review: fail on request, and keep the line for the tests to read.
    if capture::FAIL_OUT.with(std::cell::Cell::get) {
        return Err(std::io::ErrorKind::BrokenPipe.into());
    }
    println!("{line}");
    capture::OUT.with(|v| v.borrow_mut().push(line.to_string()));
    Ok(())
}

/// Write a line to standard error, dropping it if standard error is gone (a closed pipe):
/// the exit status still says what happened.
#[cfg(not(test))]
fn to_stderr(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

/// Under test, through `eprintln!`, which the test harness captures.
#[cfg(test)]
fn to_stderr(line: std::fmt::Arguments<'_>) {
    eprintln!("{line}");
    // Mutation review: keep the line for the tests to read.
    capture::ERR.with(|v| v.borrow_mut().push(line.to_string()));
}

/// End an example: print its error, if any, as one cleaned line (not `Debug`), and exit
/// non-zero on failure. Output cut short by a closed pipe is not a failure.
pub fn finish(result: Result<(), Box<dyn std::error::Error>>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            let mut printed = 0;
            report(&mut printed, || {
                format!("error: {}", printable_error(&e.to_string()))
            });
            std::process::ExitCode::FAILURE
        }
    }
}

/// Error lines printed per run before the rest are only counted, so a file with millions of
/// broken objects cannot flood the terminal.
pub const MAX_REPORTED: usize = 50;

/// Longest error line printed, in characters; the rest is cut.
pub const MAX_LINE: usize = 300;

/// Print one error line (built only if it will be printed, and cut to [`MAX_LINE`]), or count
/// it once [`MAX_REPORTED`] have been printed.
pub fn report(printed: &mut usize, line: impl FnOnce() -> String) {
    if *printed < MAX_REPORTED {
        let line = line();
        match line.char_indices().nth(MAX_LINE) {
            Some((cut, _)) => to_stderr(format_args!("{}...", &line[..cut])),
            None => to_stderr(format_args!("{line}")),
        }
    } else if *printed == MAX_REPORTED {
        to_stderr(format_args!("(further errors are counted, not printed)"));
    }
    *printed += 1;
}

/// The crate's test file builders.
#[cfg(test)]
#[path = "../../tests/common/mod.rs"]
mod builders;

/// Files for the examples' own tests: each example is run on them under `cargo test`, so an
/// example that stops working fails the build rather than a user.
#[cfg(test)]
pub mod fixture {
    use super::builders;
    pub use builders::TempDir;
    use builders::{format, serialized, sprite, texture, Mesh, Pixels, SPRITE, TEXTURE_2D};

    /// The texture's path ID in [`sample`].
    pub const TEXTURE: i64 = 10;
    /// The sprite's path ID in [`sample`].
    pub const SPRITE_ID: i64 = 1;

    /// A Unity 2022.3 file with a 4x2 RGBA32 texture "tex" and a sprite "icon" over its lower
    /// left 2x1 pixels (neither square, so width and height cannot be swapped unseen); with
    /// `bad`, also a texture "bc7" in a format the crate does not decode.
    pub fn sample(dir: &TempDir, bad: bool) -> std::path::PathBuf {
        let rgba: Vec<u8> = (0..32).collect();
        let tex = texture(
            builders::Layout::U2022_3,
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
            TEXTURE,
            0,
            r,
            0b10,
            1.0,
            &Mesh::BASE,
        );
        let mut objects = vec![(TEXTURE, TEXTURE_2D, tex), (SPRITE_ID, SPRITE, icon)];
        if bad {
            let bc7 = texture(
                builders::Layout::U2022_3,
                false,
                "bc7",
                4,
                4,
                format::BC7,
                &Pixels::Inline(&[0; 16]),
                &[],
            );
            objects.push((20, TEXTURE_2D, bc7));
        }
        dir.file(
            "sample.assets",
            &serialized(22, "2022.3.62f1", false, 19, &objects),
        )
    }

    /// [`sample`] inside an asset bundle, as its one serialized file.
    pub fn sample_bundle(dir: &TempDir) -> std::path::PathBuf {
        let inner = std::fs::read(sample(dir, false)).unwrap();
        let bytes = builders::bundle(
            &builders::BundleOpts::new(6, "2022.3.62f1"),
            &[("CAB-sample", &inner, 4)],
        );
        dir.file("sample.bundle", &bytes)
    }

    /// Arguments as the command line gives them.
    pub fn args(list: &[&std::ffi::OsStr]) -> std::vec::IntoIter<std::ffi::OsString> {
        list.iter()
            .map(|a| a.to_os_string())
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_collide() {
        let mut names = Names::default();
        let got = [
            names.claim("a_5", 1),
            names.claim("a", 2),
            names.claim("A", 5),
            names.claim("a", 5),
        ];
        let lower: HashSet<String> = got.iter().map(|n| n.to_lowercase()).collect();
        assert_eq!(lower.len(), got.len(), "{got:?}");
    }

    #[test]
    fn printable_escapes_what_hides_or_reorders_text() {
        for bad in [
            "\x1b[2J",
            "\u{202e}",
            "\u{200b}",
            "\u{2028}",
            "\u{061c}",
            "\u{e0041}",
            "\u{ad}",
        ] {
            let p = printable(bad);
            assert!(p.chars().all(|c| c.is_ascii_graphic()), "{bad:?} -> {p:?}");
        }
        assert_eq!(printable("Icon \"x\" é"), "Icon \"x\" \\u{e9}");
        assert_eq!(printable("a\\u{202e}"), "a\\\\u{202e}");
        for blank in ["\u{3164}", "\u{115f}", "\u{2800}", "\u{5d0}"] {
            assert!(printable(blank).starts_with("\\u{"), "{blank:?}");
        }
    }

    #[test]
    fn file_names_are_safe() {
        for (raw, want) in [
            ("../x", ".._x"),
            ("CON", "_CON"),
            ("com1.txt", "_com1.txt"),
            ("", "_"),
            ("..", "_.."),
            ("-rf", "_-rf"),
        ] {
            assert_eq!(file_name(raw), want, "{raw:?}");
        }
        assert_eq!(file_name(&"x".repeat(300)).len(), 100);
    }

    #[cfg(unix)]
    #[test]
    fn save_png_writes_only_new_files() {
        let dir = std::env::temp_dir().join(format!("uba-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let image = unity_bundle_assets::Image::new(1, 1, vec![1, 2, 3, 4]).unwrap();
        let canary = dir.join("canary");
        std::fs::write(&canary, b"keep").unwrap();
        std::os::unix::fs::symlink(&canary, dir.join("link.png")).unwrap();
        std::fs::hard_link(&canary, dir.join("hard.png")).unwrap();
        for taken in ["link.png", "hard.png"] {
            assert!(save_png(&dir.join(taken), &image).is_err(), "{taken}");
        }
        assert_eq!(std::fs::read(&canary).unwrap(), b"keep");
        assert!(save_png(&dir.join("new.png"), &image).is_ok());
        assert!(
            save_png(&dir.join("new.png"), &image).is_err(),
            "no overwriting"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// What the examples print is captured under test so the tests can check it, with a switch that
// makes standard output fail as a closed pipe does; more fixtures; and the helpers' own tests.

/// Lines the examples print under test, per test thread.
#[cfg(test)]
pub mod capture {
    use std::cell::{Cell, RefCell};
    thread_local! {
        pub static OUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub static ERR: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub static FAIL_OUT: Cell<bool> = const { Cell::new(false) };
    }
    /// Standard output lines since the last call.
    pub fn out() -> Vec<String> {
        OUT.with(|v| std::mem::take(&mut *v.borrow_mut()))
    }
    /// Standard error lines since the last call.
    pub fn err() -> Vec<String> {
        ERR.with(|v| std::mem::take(&mut *v.borrow_mut()))
    }
    /// Make standard output fail as a closed pipe does, or work again.
    pub fn fail_out(fail: bool) {
        FAIL_OUT.with(|f| f.set(fail));
    }
}

#[cfg(test)]
pub mod more_fixtures {
    use super::builders::{self, format, sprite, texture, Mesh, Pixels, SPRITE, TEXTURE_2D};
    use super::fixture::TempDir;
    pub use super::fixture::TEXTURE;

    /// A 4x4 RGBA32 texture holding bytes 0..64.
    pub fn rgba_texture(name: &str) -> Vec<u8> {
        let rgba: Vec<u8> = (0..64).collect();
        texture(
            builders::Layout::U2022_3,
            false,
            name,
            4,
            4,
            format::RGBA32,
            &Pixels::Inline(&rgba),
            &[],
        )
    }

    /// A sprite over the lower left 2x2 pixels of texture `texture_id`.
    pub fn sprite_on(name: &str, texture_id: i64, atlas: i64, settings: u32) -> Vec<u8> {
        let r = [0.0, 0.0, 2.0, 2.0];
        sprite(
            false,
            false,
            name,
            r,
            [0.0, 0.0],
            1,
            atlas,
            texture_id,
            0,
            r,
            settings,
            1.0,
            &Mesh::BASE,
        )
    }

    /// A Unity 2022.3 file of `objects` (`(path_id, class_id, data)`) naming `externals`.
    pub fn file(
        dir: &TempDir,
        name: &str,
        objects: &[(i64, i32, Vec<u8>)],
        externals: &[String],
    ) -> std::path::PathBuf {
        let extras = builders::Extras {
            externals: externals.to_vec(),
            ..Default::default()
        };
        dir.file(
            name,
            &builders::serialized_with(22, "2022.3.62f1", false, 19, objects, &extras),
        )
    }

    /// Texture 10 "tex" and the given sprites.
    pub fn sprites(dir: &TempDir, sprites: Vec<(i64, Vec<u8>)>) -> std::path::PathBuf {
        let mut objects = vec![(TEXTURE, TEXTURE_2D, rgba_texture("tex"))];
        objects.extend(sprites.into_iter().map(|(id, data)| (id, SPRITE, data)));
        file(dir, "more.assets", &objects, &[])
    }

    /// Textures by path ID and name.
    pub fn textures(dir: &TempDir, textures: &[(i64, &str)]) -> std::path::PathBuf {
        let objects: Vec<_> = textures
            .iter()
            .map(|&(id, name)| (id, TEXTURE_2D, rgba_texture(name)))
            .collect();
        file(dir, "textures.assets", &objects, &[])
    }

    /// Bytes the reader cannot take as a sprite.
    pub fn broken() -> Vec<u8> {
        vec![0; 3]
    }

    /// A PNG as RGBA8 with its size.
    pub fn png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
        let img = image::open(path).unwrap().to_rgba8();
        (img.width(), img.height(), img.into_raw())
    }
}

#[cfg(test)]
mod output {
    use super::*;
    use std::process::ExitCode;

    /// A writer that takes `left` bytes, then fails as a full disk does.
    struct FullAfter<W> {
        inner: W,
        left: usize,
    }

    impl<W: std::io::Write> std::io::Write for FullAfter<W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.left == 0 {
                return Err(std::io::Error::other("no space left"));
            }
            let n = buf.len().min(self.left);
            self.left -= n;
            self.inner.write(&buf[..n])
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    #[test]
    fn a_png_that_cannot_be_written_whole_is_removed() {
        let dir = fixture::TempDir::new("full");
        let image = unity_bundle_assets::Image::new(2, 1, vec![9; 8]).unwrap();
        let path = dir.0.join("x.png");
        let e = save_png_through(&path, &image, |f| FullAfter { inner: f, left: 10 }).unwrap_err();
        assert!(e.to_string().contains("no space left"), "{e}");
        assert!(!path.exists(), "the damaged file was left behind");
        // And the name is free for the next try.
        save_png(&path, &image).unwrap();
    }

    #[test]
    fn finish_maps_results_to_exit_codes_and_prints_one_clean_line() {
        let _ = capture::err();
        assert_eq!(finish(Ok(())), ExitCode::SUCCESS);
        assert!(capture::err().is_empty());
        let pipe = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        assert_eq!(finish(Err(pipe.into())), ExitCode::SUCCESS, "a closed pipe");
        assert!(capture::err().is_empty(), "a closed pipe is not reported");
        let other = std::io::Error::other("disk \u{1b}[2J full");
        assert_eq!(
            finish(Err(other.into())),
            ExitCode::FAILURE,
            "other I/O errors"
        );
        assert_eq!(capture::err(), ["error: disk \\u{1b}[2J full"]);
        assert_eq!(finish(Err("bad".into())), ExitCode::FAILURE);
        assert_eq!(capture::err(), ["error: bad"]);
    }

    #[test]
    fn printable_escapes_del_and_printable_error_keeps_backslashes() {
        assert_eq!(printable("a\x7fb"), "a\\u{7f}b");
        assert_eq!(
            printable_error("a\\b \u{1b}\u{e9}\x7f"),
            "a\\b \\u{1b}\\u{e9}\\u{7f}"
        );
    }

    #[test]
    fn cut_keeps_exactly_max_line_chars() {
        let long = "\u{e9}".repeat(MAX_LINE + 5);
        assert_eq!(cut(&long).chars().count(), MAX_LINE);
        let exact = "x".repeat(MAX_LINE);
        assert_eq!(cut(&exact), exact);
        assert_eq!(cut("short"), "short");
    }

    #[test]
    fn report_cuts_long_lines_and_stops_after_max_reported() {
        let _ = capture::err();
        let mut printed = 0;
        report(&mut printed, || "y".repeat(MAX_LINE + 50));
        assert_eq!(capture::err(), [format!("{}...", "y".repeat(MAX_LINE))]);
        let (mut printed, mut built) = (0, 0);
        for i in 0..MAX_REPORTED + 5 {
            report(&mut printed, || {
                built += 1;
                format!("e{i}")
            });
        }
        let lines = capture::err();
        assert_eq!(lines.len(), MAX_REPORTED + 1, "{lines:?}");
        assert_eq!(lines[MAX_REPORTED - 1], format!("e{}", MAX_REPORTED - 1));
        assert_eq!(
            lines[MAX_REPORTED],
            "(further errors are counted, not printed)"
        );
        assert_eq!(built, MAX_REPORTED, "lines past the cap are not built");
        assert_eq!(printed, MAX_REPORTED + 5, "but they are counted");
    }

    #[test]
    fn file_names_refuse_every_device_all_dot_and_non_ascii_name() {
        for (raw, want) in [
            ("LPT1", "_LPT1"),
            ("lpt9.png", "_lpt9.png"),
            ("nul", "_nul"),
            ("aux.x", "_aux.x"),
            (".", "_."),
            ("...", "_..."),
            ("\u{e9}", "_"),
            ("a/b\\c:d", "a_b_c_d"),
            ("COM10", "COM10"),
            ("icon", "icon"),
        ] {
            assert_eq!(file_name(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn names_append_the_path_id_then_a_counter() {
        let mut names = Names::default();
        assert_eq!(names.claim("a", 5), "a");
        assert_eq!(names.claim("A", 7), "A_7");
        assert_eq!(names.claim("a", 7), "a_7_2");
        assert_eq!(names.claim("a", 7), "a_7_3");
    }

    #[cfg(unix)]
    #[test]
    fn text_arg_refuses_non_utf8() {
        use std::os::unix::ffi::OsStringExt;
        let mut args = vec![std::ffi::OsString::from_vec(vec![b'a', 0xff])].into_iter();
        assert_eq!(
            text_arg(&mut args, "the thing").unwrap_err(),
            "the thing is not UTF-8"
        );
        assert_eq!(text_arg(&mut args, "x"), Ok(None));
    }
}
