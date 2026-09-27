//! Export sprites to PNG: `cargo run --example export -- <file> <out-dir> [name-prefix...]`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: export <file> <out-dir> [name-prefix...]")?;
    let out = std::path::PathBuf::from(args.next().ok_or("out dir")?);
    let prefixes: Vec<String> = args.collect();
    std::fs::create_dir_all(&out)?;
    let mut assets = unity_sprites::Assets::open(path.as_ref())?;
    let sprites =
        assets.sprites(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)))?;
    let (mut ok, mut failed) = (0, 0);
    for sprite in &sprites {
        match assets.export(sprite) {
            Ok(img) => {
                image::save_buffer(
                    out.join(format!("{}.png", sprite.name)),
                    &img.rgba,
                    img.width,
                    img.height,
                    image::ColorType::Rgba8,
                )?;
                ok += 1;
            }
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", sprite.name);
            }
        }
    }
    println!("exported {ok}, failed {failed}");
    Ok(())
}
