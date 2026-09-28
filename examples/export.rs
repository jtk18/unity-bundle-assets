//! Export sprites to PNG: `cargo run --example export -- <file> <out-dir> [name-prefix...]`.
//! A name used by more than one sprite (ignoring case) gets its path ID appended.
//!
//! Files already in the output folder are never overwritten (each is reported instead), so
//! export into an empty folder. The output is bounded only by the file's work limit: a small
//! hostile file can ask for tens of gigabytes of PNG. This example uses the default limits:
//! watch the disk when exporting files you did not make.

mod common;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: export <file> <out-dir> [name-prefix...]")?;
    let out = std::path::PathBuf::from(args.next().ok_or("out dir")?);
    let mut prefixes = Vec::new();
    while let Some(prefix) = common::text_arg(&mut args, "a name prefix")? {
        prefixes.push(prefix);
    }
    std::fs::create_dir_all(&out)?;
    let mut assets = unity_bundle_assets::Assets::open(&path)?;
    let list = assets.sprites(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)));
    let mut names = common::Names::default();
    let (mut ok, mut failed) = (0, list.skipped.len());
    let mut printed = 0;
    for skipped in &list.skipped {
        common::report(&mut printed, || {
            format!(
                "{} ({}): {}",
                common::printable(skipped.name.as_deref().unwrap_or("?")),
                skipped.path_id,
                common::printable(&skipped.error.to_string())
            )
        });
    }
    for sprite in &list.sprites {
        let saved = assets
            .export(sprite)
            .map_err(|e| e.to_string())
            .and_then(|img| {
                let name = names.claim(&common::file_name(&sprite.name), sprite.path_id);
                common::save_png(&out.join(format!("{name}.png")), &img).map_err(|e| e.to_string())
            });
        match saved {
            Ok(()) => ok += 1,
            Err(e) => {
                failed += 1;
                common::report(&mut printed, || {
                    format!(
                        "{} ({}): {}",
                        common::printable(&sprite.name),
                        sprite.path_id,
                        common::printable(&e)
                    )
                });
            }
        }
    }
    println!("exported {ok}, failed {failed}");
    Ok(())
}
