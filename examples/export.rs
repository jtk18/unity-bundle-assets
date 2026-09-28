//! Export sprites to PNG: `cargo run --example export -- <file> <out-dir> [name-prefix...]`.
//! A name used by more than one sprite gets its path ID appended after the first.

mod common;

use std::collections::HashSet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: export <file> <out-dir> [name-prefix...]")?;
    let out = std::path::PathBuf::from(args.next().ok_or("out dir")?);
    let prefixes: Vec<String> = args.collect();
    std::fs::create_dir_all(&out)?;
    let mut assets = unity_bundle_assets::Assets::open(&path)?;
    let list = assets.sprites(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)));
    let mut used = HashSet::new();
    let (mut ok, mut failed) = (0, list.skipped.len());
    for skipped in &list.skipped {
        eprintln!(
            "{} ({}): {}",
            common::printable(skipped.name.as_deref().unwrap_or("?")),
            skipped.path_id,
            skipped.error
        );
    }
    for sprite in &list.sprites {
        match assets.export(sprite) {
            Ok(img) => {
                let mut name = common::file_name(&sprite.name);
                if !used.insert(name.clone()) {
                    name = format!("{name}_{}", sprite.path_id);
                }
                common::save_png(&out.join(format!("{name}.png")), &img)?;
                ok += 1;
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "{} ({}): {e}",
                    common::printable(&sprite.name),
                    sprite.path_id
                );
            }
        }
    }
    println!("exported {ok}, failed {failed}");
    Ok(())
}
