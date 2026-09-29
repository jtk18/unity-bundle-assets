//! The README's example command lines agree with the examples themselves, and name them all.
//! (The README's Rust snippets are compiled as doc tests; see `ReadmeDoctests` in lib.rs.)

use std::collections::BTreeMap;
use std::path::Path;

/// Each example's `USAGE`, by name: the part after `usage: <name> `.
fn usages() -> BTreeMap<String, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let source = std::fs::read_to_string(&path).unwrap();
        let marker = format!("const USAGE: &str = \"usage: {name} ");
        let at = source
            .find(&marker)
            .unwrap_or_else(|| panic!("examples/{name}.rs has no `USAGE` of its own name"));
        let rest = &source[at + marker.len()..];
        found.insert(name, rest[..rest.find('"').unwrap()].to_string());
    }
    found
}

/// Each README line `cargo run [--release] --example <name> -- <args>  # ...`, by name.
fn readme_lines() -> BTreeMap<String, String> {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    let mut found = BTreeMap::new();
    for line in readme.lines().filter(|l| l.starts_with("cargo run ")) {
        let line = line.split(" # ").next().unwrap().trim_end();
        let (head, args) = line.split_once(" -- ").expect("an example line has ` -- `");
        let name = head.rsplit(' ').next().unwrap().to_string();
        assert!(head.ends_with(&format!("--example {name}")), "{line}");
        assert!(
            found.insert(name.clone(), args.to_string()).is_none(),
            "{name} is listed twice"
        );
    }
    found
}

#[test]
fn the_readme_lists_every_example_with_its_own_usage() {
    let (usages, readme) = (usages(), readme_lines());
    assert!(!usages.is_empty());
    assert_eq!(
        readme.keys().collect::<Vec<_>>(),
        usages.keys().collect::<Vec<_>>(),
        "the README's examples and examples/*.rs differ"
    );
    for (name, args) in &readme {
        assert_eq!(args, &usages[name], "README line for {name}");
    }
}

#[test]
fn each_examples_doc_line_matches_its_usage() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    for (name, usage) in usages() {
        let source = std::fs::read_to_string(dir.join(format!("{name}.rs"))).unwrap();
        let docs: String = source
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            docs.contains(&format!("--example {name} -- {usage}`")),
            "examples/{name}.rs's doc comment does not show `-- {usage}`"
        );
    }
}
