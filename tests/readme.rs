//! The README and the examples agree, and every example is tested: its command line in the
//! README matches its own `USAGE` and doc comment, it runs its tests under `cargo test`, and
//! the README's Rust snippets are compiled (as doc tests; see `ReadmeDoctests` in lib.rs).

use std::collections::BTreeMap;
use std::path::Path;

fn read(file: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap()
}

/// The `[[example]]` tables of Cargo.toml, as their trimmed `key = value` lines.
fn example_tables() -> Vec<Vec<String>> {
    read("Cargo.toml")
        .split("\n[")
        .filter(|t| t.starts_with("[example]]"))
        .map(|t| t.lines().skip(1).map(|l| l.trim().to_string()).collect())
        .collect()
}

/// Every example: those Cargo.toml declares, which must be those in examples/*.rs.
fn example_names() -> Vec<String> {
    let mut declared: Vec<String> = example_tables()
        .iter()
        .filter_map(|t| {
            t.iter()
                .find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect();
    declared.sort();
    let mut files: Vec<String> =
        std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("examples"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
            .collect();
    files.sort();
    assert_eq!(
        declared, files,
        "Cargo.toml's [[example]]s and examples/*.rs differ"
    );
    declared
}

/// Each example's `USAGE`, by name: the part after `usage: <name> `.
fn usages() -> BTreeMap<String, String> {
    example_names()
        .into_iter()
        .map(|name| {
            let source = read(&format!("examples/{name}.rs"));
            let marker = format!("const USAGE: &str = \"usage: {name} ");
            let at = source
                .find(&marker)
                .unwrap_or_else(|| panic!("examples/{name}.rs has no `USAGE` of its own name"));
            let rest = &source[at + marker.len()..];
            let usage = &rest[..rest.find('"').unwrap()];
            assert!(
                !usage.contains('\\'),
                "examples/{name}.rs: keep USAGE free of escapes"
            );
            (name, usage.to_string())
        })
        .collect()
}

/// The README's fenced blocks: (info string, body lines).
fn blocks(readme: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut open: Option<(String, Vec<String>)> = None;
    for line in readme.lines() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            match open.take() {
                Some(block) => out.push(block),
                None => open = Some((info.trim().to_string(), Vec::new())),
            }
        } else if let Some((_, body)) = open.as_mut() {
            body.push(line.to_string());
        }
    }
    assert!(open.is_none(), "an unclosed code block in the README");
    out
}

/// Each README example command (`cargo run [--release] --example <name> -- <args>`, less any
/// trailing `# comment`), by name. Every line that mentions `--example` must be one, in the
/// one `sh` block that holds them all.
fn readme_commands() -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut blocks_holding = Vec::new();
    for (i, (info, body)) in blocks(&read("README.md")).iter().enumerate() {
        for line in body.iter().filter(|l| l.contains("--example")) {
            assert_eq!(
                info, "sh",
                "an example command outside the sh block: {line:?}"
            );
            blocks_holding.push(i);
            let command = line.split(" # ").next().unwrap().trim_end();
            let (head, _) = command
                .split_once(" -- ")
                .unwrap_or_else(|| panic!("no ` -- ` in {line:?}"));
            let name = head.rsplit(' ').next().unwrap();
            assert!(
                head == format!("cargo run --example {name}")
                    || head == format!("cargo run --release --example {name}"),
                "not a plain `cargo run [--release] --example <name> -- ...` line: {line:?}"
            );
            assert!(
                found
                    .insert(name.to_string(), command.to_string())
                    .is_none(),
                "{name} is listed twice"
            );
        }
    }
    blocks_holding.dedup();
    assert_eq!(
        blocks_holding.len(),
        1,
        "the example commands are split across code blocks"
    );
    found
}

#[test]
fn the_readme_lists_every_example_with_its_own_usage() {
    let (usages, readme) = (usages(), readme_commands());
    assert!(!usages.is_empty());
    assert_eq!(
        readme.keys().collect::<Vec<_>>(),
        usages.keys().collect::<Vec<_>>(),
        "the README's examples and examples/*.rs differ"
    );
    for (name, command) in &readme {
        let args = command.split_once(" -- ").unwrap().1;
        assert_eq!(args, usages[name], "README line for {name}");
    }
}

#[test]
fn each_examples_doc_comment_shows_the_readmes_command() {
    for (name, command) in readme_commands() {
        let source = read(&format!("examples/{name}.rs"));
        let docs: String = source
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//!").trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            docs.contains(&format!("`{command}`")),
            "examples/{name}.rs's doc comment does not show `{command}` (in backticks)"
        );
    }
}

#[test]
fn every_example_runs_its_tests_under_cargo_test() {
    let tables = example_tables();
    for name in example_names() {
        let table = tables
            .iter()
            .find(|t| t.contains(&format!("name = \"{name}\"")))
            .unwrap();
        assert!(
            table.iter().any(|l| l == "test = true"),
            "examples/{name}.rs's tests do not run under `cargo test` (set `test = true`)"
        );
    }
}

#[test]
fn every_rust_snippet_is_compiled() {
    assert!(
        read("src/lib.rs").contains("#[cfg(doctest)]\n#[doc = include_str!(\"../README.md\")]"),
        "src/lib.rs no longer compiles the README as doc tests"
    );
    let mut rust = 0;
    for (info, body) in blocks(&read("README.md")) {
        if body
            .iter()
            .any(|l| l.contains("unity_bundle_assets") || l.contains("fn main"))
        {
            let tags: Vec<&str> = info.split(',').map(str::trim).collect();
            assert_eq!(
                tags[0], "rust",
                "a Rust snippet in a ```{info} block is not compiled"
            );
            assert!(
                !tags
                    .iter()
                    .any(|t| ["ignore", "text", "compile_fail"].contains(t)),
                "a ```{info} snippet is not compiled"
            );
            rust += 1;
        }
    }
    assert!(rust >= 2, "the README's Rust snippets went missing");
}
