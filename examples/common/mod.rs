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
    /// `base`, or `base_<id>` if a name equal to it ignoring case was already handed out.
    pub fn claim(&mut self, base: &str, id: i64) -> String {
        let mut name = base.to_string();
        if !self.0.insert(name.to_lowercase()) {
            name = format!("{base}_{id}");
            self.0.insert(name.to_lowercase());
        }
        name
    }
}

/// A string safe to print to a terminal: control characters (escape sequences included) and
/// the invisible characters that reorder or hide text are shown escaped.
pub fn printable(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let hidden = matches!(c as u32,
                0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0xFEFF);
            if c.is_control() || hidden {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Write a top-down RGBA image as PNG.
pub fn save_png(
    path: &std::path::Path,
    image: &unity_bundle_assets::Image,
) -> Result<(), Box<dyn std::error::Error>> {
    image::save_buffer(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8,
    )?;
    Ok(())
}
