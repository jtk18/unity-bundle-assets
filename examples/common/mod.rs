//! Helpers the examples share. Names come from the file being read, so they are cleaned
//! before they touch a path or a terminal.

#![allow(dead_code)]

/// A name reduced to one safe path component: letters, digits, `.`, `_` and `-`, never empty
/// and never `.` or `..`.
pub fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.chars().all(|c| c == '.') {
        format!("_{safe}")
    } else {
        safe
    }
}

/// A string safe to print to a terminal: control characters (escape sequences included)
/// are shown escaped.
pub fn printable(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_control() {
                c.escape_default().collect::<Vec<_>>()
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
