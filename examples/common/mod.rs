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
    use image::ImageEncoder;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    // Flushed here, not on drop, so a disk that fills up at the end is an error, not a
    // silently truncated PNG.
    let mut out = std::io::BufWriter::new(file);
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
    println!("{line}");
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

    /// A Unity 2022.3 file with a 4x4 RGBA32 texture "tex" and a sprite "icon" over its lower
    /// left 2x2 pixels; with `bad`, also a texture "bc7" in a format the crate does not
    /// decode.
    pub fn sample(dir: &TempDir, bad: bool) -> std::path::PathBuf {
        let rgba: Vec<u8> = (0..64).collect();
        let tex = texture(
            builders::Layout::U2022_3,
            false,
            "tex",
            4,
            4,
            format::RGBA32,
            &Pixels::Inline(&rgba),
            &[],
        );
        let r = [0.0, 0.0, 2.0, 2.0];
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
