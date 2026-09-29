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
    use common::fixture::{args, sample, sample_bundle, TempDir, TEXTURE};

    #[test]
    fn exports_from_a_bundle() {
        let dir = TempDir::new("ex-textures-bundle");
        let out = dir.0.join("out");
        run(args(&[sample_bundle(&dir).as_os_str(), out.as_os_str()])).unwrap();
        assert!(out.join(format!("tex_{TEXTURE}.png")).exists());
    }

    #[test]
    fn exports_the_textures_as_pngs() {
        let dir = TempDir::new("ex-textures");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        let png = image::open(out.join(format!("tex_{TEXTURE}.png"))).unwrap();
        assert_eq!((png.width(), png.height()), (4, 2));
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

/// Added by the mutation review: what `textures` writes, selects and prints.
#[cfg(test)]
mod output {
    use super::*;
    use common::capture;
    use common::fixture::{args, sample, TempDir, TEXTURE};
    use common::more_fixtures::{self, png};
    use std::ffi::OsStr;

    #[test]
    fn the_png_holds_the_decoded_texture() {
        let dir = TempDir::new("mk-tex-px");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        let _ = capture::out();
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        assert_eq!(capture::out(), ["exported 1, failed 0"]);
        let want = unity_bundle_assets::Assets::open(&file)
            .unwrap()
            .decode_texture(TEXTURE)
            .unwrap();
        assert_eq!(
            png(&out.join(format!("tex_{TEXTURE}.png"))),
            (want.width(), want.height(), want.rgba().to_vec())
        );
    }

    #[test]
    fn a_prefix_selects_textures() {
        let dir = TempDir::new("mk-tex-prefix");
        let file = sample(&dir, true);
        let out = dir.0.join("out");
        // "te" leaves out the texture it cannot decode, so the run succeeds.
        run(args(&[
            file.as_os_str(),
            out.as_os_str(),
            OsStr::new("zz"),
            OsStr::new("te"),
        ]))
        .unwrap();
        let files: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(files, [format!("tex_{TEXTURE}.png")]);
    }

    #[test]
    fn a_failure_is_counted_and_reported() {
        let dir = TempDir::new("mk-tex-fail");
        let file = sample(&dir, true);
        let (_, _) = (capture::out(), capture::err());
        assert!(run(args(&[file.as_os_str(), dir.0.join("out").as_os_str()])).is_err());
        assert_eq!(capture::out(), ["exported 1, failed 1"]);
        let err = capture::err();
        assert_eq!(err.len(), 1, "{err:?}");
        assert!(err[0].starts_with("astc (20): "), "{err:?}");
    }

    #[test]
    fn names_are_cleaned() {
        let dir = TempDir::new("mk-tex-names");
        let file = more_fixtures::textures(&dir, &[(7, "../up")]);
        let out = dir.0.join("out");
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        assert!(out.join(".._up_7.png").exists());
    }

    #[test]
    fn a_missing_input_makes_no_folder() {
        let dir = TempDir::new("mk-tex-missing");
        let out = dir.0.join("out");
        assert!(run(args(&[dir.0.join("missing").as_os_str(), out.as_os_str()])).is_err());
        assert!(!out.exists());
    }
}
