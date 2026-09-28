//! List a serialized file's objects:
//! `cargo run --example list -- <file-or-bundle> [class-id]`.

#[macro_use]
mod common;

use std::collections::BTreeMap;

fn main() -> std::process::ExitCode {
    common::finish(run())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: list <file-or-bundle> [class-id]")?;
    let filter: Option<i32> = common::text_arg(&mut args, "the class id")?
        .map(|s| s.parse())
        .transpose()?;
    let assets = unity_bundle_assets::Assets::open(&path)?;
    let file = assets.file();
    outln!(
        "format {} unity {} platform {} type trees {} externals {}",
        file.version(),
        common::printable(file.unity_version()),
        file.target_platform(),
        file.has_type_trees(),
        file.externals().len()
    );
    // One line each, cut, and only so many: a file can name millions of long paths.
    let externals = file.externals();
    for e in externals.iter().take(common::MAX_REPORTED) {
        outln!("  external {}", cut(&common::printable(&e.path)));
    }
    if externals.len() > common::MAX_REPORTED {
        outln!(
            "  ({} more externals)",
            externals.len() - common::MAX_REPORTED
        );
    }
    let mut counts = BTreeMap::new();
    for o in file.objects() {
        *counts.entry(o.class_id()).or_insert(0) += 1;
        if Some(o.class_id()) == filter {
            outln!(
                "{:>20} {:>9} {}",
                o.path_id(),
                o.size(),
                cut(&common::printable(&file.name(o).unwrap_or_default()))
            );
        }
    }
    // One class a line, and only so many: a file can hold millions of classes.
    let classes = counts.len();
    for (class, n) in counts.into_iter().take(common::MAX_REPORTED) {
        outln!("class {class:>6}: {n}");
    }
    if classes > common::MAX_REPORTED {
        outln!("({} more classes)", classes - common::MAX_REPORTED);
    }
    Ok(())
}

/// `s` cut to [`common::MAX_LINE`] characters.
fn cut(s: &str) -> &str {
    s.char_indices()
        .nth(common::MAX_LINE)
        .map_or(s, |(at, _)| &s[..at])
}
