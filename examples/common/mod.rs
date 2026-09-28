//! Helpers the examples share. Names come from the file being read, so they are cleaned
//! before they touch a path or a terminal.

#![allow(dead_code)]

use std::collections::HashSet;

/// A name reduced to one safe path component: letters, digits, `.`, `_` and `-`, at most 100
/// bytes, never empty, never `.` or `..`, and never a Windows device name.
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
    if safe.is_empty() || safe.chars().all(|c| c == '.') || device {
        safe.insert(0, '_');
    }
    safe
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

/// A string safe to print to a terminal: every character Rust's `Debug` would escape
/// (control characters, and the invisible ones that reorder or hide text) is shown escaped;
/// quotes and backslashes are left alone.
pub fn printable(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if matches!(c, '"' | '\'' | '\\') {
                vec![c]
            } else {
                c.escape_debug().collect()
            }
        })
        .collect()
}

/// Write a top-down RGBA image as PNG.
pub fn save_png(
    path: &std::path::Path,
    image: &unity_bundle_assets::Image,
) -> Result<(), Box<dyn std::error::Error>> {
    // Writing through a link left in the output folder would overwrite whatever it points at.
    if path.is_symlink() {
        return Err(format!("{} is a symbolic link", path.display()).into());
    }
    image::save_buffer(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8,
    )?;
    Ok(())
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
        assert_eq!(printable("Icon \"x\" é"), "Icon \"x\" é");
    }

    #[test]
    fn file_names_are_safe() {
        for (raw, want) in [
            ("../x", ".._x"),
            ("CON", "_CON"),
            ("com1.txt", "_com1.txt"),
            ("", "_"),
            ("..", "_.."),
        ] {
            assert_eq!(file_name(raw), want, "{raw:?}");
        }
        assert_eq!(file_name(&"x".repeat(300)).len(), 100);
    }
}
