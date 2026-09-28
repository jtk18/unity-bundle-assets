//! Read every sprite and count them by texture format, packing, and atlas use:
//! `cargo run --example survey -- <file>`.

use std::collections::BTreeMap;
use unity_bundle_assets::{class, SerializedFile, Sprite, SpriteAtlas, Texture2D};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: survey <file>")?;
    let file = SerializedFile::open(path.as_ref())?;
    let atlases: BTreeMap<i64, SpriteAtlas> = file
        .objects
        .iter()
        .filter(|o| o.class_id == class::SPRITE_ATLAS)
        .map(|o| Ok((o.path_id, SpriteAtlas::read(&file, o)?)))
        .collect::<Result<_, unity_bundle_assets::Error>>()?;
    let mut textures = BTreeMap::new();
    let (mut ok, mut failed) = (0, 0);
    let mut by = BTreeMap::new();
    for o in file.objects.iter().filter(|o| o.class_id == class::SPRITE) {
        let sprite = match Sprite::read(&file, o) {
            Ok(s) => s,
            Err(e) => {
                failed += 1;
                if failed < 5 {
                    eprintln!("sprite {}: {e}", o.path_id);
                }
                continue;
            }
        };
        ok += 1;
        let placement = atlases
            .get(&sprite.atlas.path_id)
            .and_then(|a| a.placement(&sprite.render_data_key))
            .unwrap_or(sprite.own);
        let tex = if placement.texture.file_id != 0 {
            format!("external {}", placement.texture.file_id)
        } else {
            let t = textures
                .entry(placement.texture.path_id)
                .or_insert_with(|| {
                    file.object(placement.texture.path_id).map(|obj| {
                        Texture2D::read(&file, obj)
                            .map(|t| t.format)
                            .map_err(|e| e.to_string())
                    })
                });
            format!("{t:?}")
        };
        *by.entry((
            tex,
            placement.settings.tight(),
            format!("{:?}", placement.settings.rotation()),
            !sprite.atlas.is_null(),
        ))
        .or_insert(0) += 1;
    }
    println!("sprites read {ok}, failed {failed}");
    for (k, n) in by {
        println!("{n:>6} {k:?}");
    }
    Ok(())
}
