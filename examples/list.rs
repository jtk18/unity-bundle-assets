//! List a serialized file's objects:
//! `cargo run --example list -- <file-or-bundle> [class-id]`.

#[macro_use]
mod common;

use std::collections::BTreeMap;

/// How to call it; the README's line for this example must match.
const USAGE: &str = "usage: list <file-or-bundle> [class-id]";

fn main() -> std::process::ExitCode {
    common::finish(run(std::env::args_os().skip(1)))
}

fn run(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.next().ok_or(USAGE)?;
    let filter: Option<i32> = common::text_arg(&mut args, "the class id")?
        .map(|s| s.parse().map_err(|e| format!("the class id: {e}")))
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
        outln!("  external {}", common::printable(common::cut(&e.path)));
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
                // Cut before escaping, so a long name costs no more than a short one.
                common::printable(common::cut(&file.name(o).unwrap_or_default()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::fixture::{args, sample, sample_bundle, TempDir};
    use std::ffi::OsStr;

    #[test]
    fn lists_a_bundle() {
        let dir = TempDir::new("ex-list-bundle");
        run(args(&[sample_bundle(&dir).as_os_str()])).unwrap();
    }

    #[test]
    fn lists_a_file_and_a_class() {
        let dir = TempDir::new("ex-list");
        let file = sample(&dir, false);
        run(args(&[file.as_os_str()])).unwrap();
        run(args(&[file.as_os_str(), OsStr::new("28")])).unwrap();
    }

    #[test]
    fn says_what_is_wrong_with_its_arguments() {
        assert_eq!(run(args(&[])).unwrap_err().to_string(), USAGE);
        let dir = TempDir::new("ex-list-bad");
        let file = sample(&dir, false);
        let e = run(args(&[file.as_os_str(), OsStr::new("x")])).unwrap_err();
        assert!(e.to_string().starts_with("the class id"), "{e}");
    }
}

/// Added by the mutation review: the original tests ran `list` but never read its output.
#[cfg(test)]
mod output {
    use super::*;
    use common::capture;
    use common::fixture::{args, sample, TempDir, TEXTURE};
    use common::more_fixtures;
    use std::ffi::OsStr;

    const HEADER: &str = "format 22 unity 2022.3.62f1 platform 19 type trees false externals 0";

    #[test]
    fn prints_the_header_the_counts_and_only_the_chosen_class() {
        let dir = TempDir::new("mk-list");
        let file = sample(&dir, false);
        let _ = capture::out();
        run(args(&[file.as_os_str()])).unwrap();
        assert_eq!(
            capture::out(),
            [HEADER, "class     28: 1", "class    213: 1"]
        );
        let assets = unity_bundle_assets::Assets::open(&file).unwrap();
        let size = assets.file().object(TEXTURE).unwrap().size();
        run(args(&[file.as_os_str(), OsStr::new("28")])).unwrap();
        assert_eq!(
            capture::out(),
            [
                HEADER.to_string(),
                format!("{TEXTURE:>20} {size:>9} tex"),
                "class     28: 1".into(),
                "class    213: 1".into(),
            ]
        );
    }

    #[test]
    fn externals_are_escaped_cut_and_capped() {
        let dir = TempDir::new("mk-list-ext");
        let mut externals = vec!["a\u{1b}b".to_string(), "x".repeat(400)];
        externals.extend((0..common::MAX_REPORTED - 1).map(|i| format!("lib{i}")));
        let file = more_fixtures::file(&dir, "ext.assets", &[], &externals);
        let _ = capture::out();
        run(args(&[file.as_os_str()])).unwrap();
        let out = capture::out();
        assert!(out[0].ends_with(&format!("externals {}", common::MAX_REPORTED + 1)));
        assert_eq!(out[1], "  external a\\u{1b}b");
        assert_eq!(
            out[2],
            format!("  external {}", "x".repeat(common::MAX_LINE))
        );
        assert_eq!(
            out.iter().filter(|l| l.starts_with("  external ")).count(),
            common::MAX_REPORTED
        );
        assert_eq!(out[common::MAX_REPORTED + 1], "  (1 more externals)");
    }

    #[test]
    fn a_file_it_cannot_open_is_an_error() {
        let dir = TempDir::new("mk-list-missing");
        assert!(run(args(&[dir.0.join("missing").as_os_str()])).is_err());
    }
}
