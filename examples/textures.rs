//! Export textures to PNG:
//! `cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]`.
//!
//! Files already in the output folder are never overwritten (each is reported instead), so
//! export into an empty folder. The output is bounded only by the file's work limit: a small
//! hostile file can ask for tens of gigabytes of PNG, in up to millions of files. This
//! example uses the default limits: watch the disk when exporting files you did not make.

#[macro_use]
mod common;

/// How to call it; the README's line for this example must match.
const USAGE: &str = "usage: textures <file-or-bundle> <out-dir> [name-prefix...]";

fn main() -> std::process::ExitCode {
    common::finish(run(std::env::args_os().skip(1)))
}

fn run(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.next().ok_or(USAGE)?;
    let out = std::path::PathBuf::from(args.next().ok_or(USAGE)?);
    let mut prefixes = Vec::new();
    while let Some(prefix) = common::text_arg(&mut args, "a name prefix")? {
        prefixes.push(prefix);
    }
    let assets = unity_bundle_assets::Assets::open(&path)?;
    // Only once the input opens, so a mistyped input path makes no folder.
    std::fs::create_dir_all(&out)?;
    let (mut ok, mut failed) = (0, 0);
    let mut printed = 0;
    for texture in
        assets.textures(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)))
    {
        let name = texture.name.as_deref().unwrap_or("");
        let saved = assets
            .decode_texture(texture.path_id)
            .map_err(|e| e.to_string())
            .and_then(|img| {
                let file = format!("{}_{}.png", common::file_name(name), texture.path_id);
                common::save_png(&out.join(file), &img).map_err(|e| e.to_string())
            });
        match saved {
            Ok(()) => ok += 1,
            Err(e) => {
                failed += 1;
                common::report(&mut printed, || {
                    format!(
                        "{} ({}): {}",
                        common::printable(common::cut(name)),
                        texture.path_id,
                        common::printable_error(&e)
                    )
                });
            }
        }
    }
    // The failures decide the exit status, even if the summary cannot be written.
    let summary = common::write_line(format_args!("exported {ok}, failed {failed}"));
    if failed > 0 {
        return Err(format!("{failed} not exported").into());
    }
    Ok(summary?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::fixture::{args, sample, TempDir, TEXTURE};

    #[test]
    fn exports_the_textures_as_pngs() {
        let dir = TempDir::new("ex-textures");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        let png = image::open(out.join(format!("tex_{TEXTURE}.png"))).unwrap();
        assert_eq!((png.width(), png.height()), (4, 4));
    }

    #[test]
    fn a_texture_it_cannot_decode_fails_the_run_but_not_the_others() {
        let dir = TempDir::new("ex-textures-bad");
        let file = sample(&dir, true);
        let out = dir.0.join("out");
        let e = run(args(&[file.as_os_str(), out.as_os_str()])).unwrap_err();
        assert_eq!(e.to_string(), "1 not exported");
        assert!(out.join(format!("tex_{TEXTURE}.png")).exists());
        assert_eq!(
            run(args(&[file.as_os_str()])).unwrap_err().to_string(),
            USAGE
        );
    }
}
