//! Read every sprite and count them by texture format, packing, and atlas use:
//! `cargo run --example survey -- <file>`.

mod common;

use std::collections::BTreeMap;
use unity_bundle_assets::Assets;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: survey <file>")?;
    let assets = Assets::open(&path)?;
    let list = assets.sprites(|_| true);
    for s in list.skipped.iter().take(5) {
        eprintln!(
            "sprite {}: {}",
            s.path_id,
            common::printable(&s.error.to_string())
        );
    }
    let mut formats = BTreeMap::new();
    let mut by = BTreeMap::new();
    for sprite in &list.sprites {
        let tex = match assets.placement(sprite) {
            Err(e) => format!("error: {}", common::printable(&e.to_string())),
            Ok(p) if p.texture.file_id != 0 => format!("external {}", p.texture.file_id),
            Ok(p) => {
                let t = formats.entry(p.texture.path_id).or_insert_with(|| {
                    assets
                        .texture(p.texture.path_id)
                        .map(|t| t.format)
                        .map_err(|e| e.to_string())
                });
                format!("{t:?}")
            }
        };
        let settings = assets
            .placement(sprite)
            .map(|p| p.settings)
            .unwrap_or_default();
        *by.entry((
            tex,
            settings.tight(),
            format!("{:?}", settings.rotation()),
            !sprite.atlas.is_null(),
        ))
        .or_insert(0) += 1;
    }
    println!(
        "sprites read {}, skipped {}",
        list.sprites.len(),
        list.skipped.len()
    );
    for (k, n) in by {
        println!("{n:>6} {k:?}");
    }
    Ok(())
}
