//! Export textures to PNG:
//! `cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: textures <file-or-bundle> <out-dir> [name-prefix...]")?;
    let out = std::path::PathBuf::from(args.next().ok_or("out dir")?);
    let prefixes: Vec<String> = args.collect();
    std::fs::create_dir_all(&out)?;
    let assets = unity_bundle_assets::Assets::open(path.as_ref())?;
    let (mut ok, mut failed) = (0, 0);
    for texture in
        assets.textures(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)))
    {
        let (path_id, name) = (texture.path_id, texture.name);
        match assets.decode_texture(path_id) {
            Ok(img) => {
                // Names come from the file: keep them to one safe path component.
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
                // decode_texture gives Unity's bottom-up rows; PNG wants top-down.
                let row = (img.width * 4) as usize;
                let top_down: Vec<u8> = img
                    .rgba
                    .chunks_exact(row)
                    .rev()
                    .flatten()
                    .copied()
                    .collect();
                image::save_buffer(
                    out.join(format!("{safe}_{path_id}.png")),
                    &top_down,
                    img.width,
                    img.height,
                    image::ColorType::Rgba8,
                )?;
                ok += 1;
            }
            Err(e) => {
                failed += 1;
                eprintln!("{name} ({path_id}): {e}");
            }
        }
    }
    println!("exported {ok}, failed {failed}");
    Ok(())
}
