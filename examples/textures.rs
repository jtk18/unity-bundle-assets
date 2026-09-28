//! Export textures to PNG:
//! `cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]`.

mod common;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: textures <file-or-bundle> <out-dir> [name-prefix...]")?;
    let out = std::path::PathBuf::from(args.next().ok_or("out dir")?);
    let prefixes: Vec<String> = args.collect();
    std::fs::create_dir_all(&out)?;
    let assets = unity_bundle_assets::Assets::open(&path)?;
    let (mut ok, mut failed) = (0, 0);
    for texture in
        assets.textures(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)))
    {
        match assets.decode_texture(texture.path_id) {
            Ok(img) => {
                let name = format!(
                    "{}_{}.png",
                    common::file_name(&texture.name),
                    texture.path_id
                );
                common::save_png(&out.join(name), &img)?;
                ok += 1;
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "{} ({}): {e}",
                    common::printable(&texture.name),
                    texture.path_id
                );
            }
        }
    }
    println!("exported {ok}, failed {failed}");
    Ok(())
}
