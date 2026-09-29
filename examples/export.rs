//! Export sprites to PNG:
//! `cargo run --release --example export -- <file-or-bundle> <out-dir> [name-prefix...]`.
//! A name used by more than one sprite (ignoring case) gets its path ID appended.
//!
//! Files already in the output folder are never overwritten (each is reported instead), so
//! export into an empty folder. The output is bounded only by the file's work limit: a small
//! hostile file can ask for tens of gigabytes of PNG, in up to millions of files. This
//! example uses the default limits: watch the disk when exporting files you did not make.

#[macro_use]
mod common;

/// How to call it; the README's line for this example must match.
const USAGE: &str = "usage: export <file-or-bundle> <out-dir> [name-prefix...]";

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
    let mut assets = unity_bundle_assets::Assets::open(&path)?;
    // Only once the input opens, so a mistyped input path makes no folder.
    std::fs::create_dir_all(&out)?;
    let list = assets.sprites(|n| prefixes.is_empty() || prefixes.iter().any(|p| n.starts_with(p)));
    let mut names = common::Names::default();
    let (mut ok, mut failed) = (0, list.skipped.len());
    let mut printed = 0;
    for skipped in &list.skipped {
        common::report(&mut printed, || {
            format!(
                "{} ({}): {}",
                common::printable(common::cut(skipped.name.as_deref().unwrap_or("?"))),
                skipped.path_id,
                common::printable_error(&skipped.error.to_string())
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
                        common::printable(common::cut(&sprite.name)),
                        sprite.path_id,
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
    use common::fixture::{args, sample, sample_bundle, TempDir};
    use std::ffi::OsStr;

    #[test]
    fn exports_from_a_bundle() {
        let dir = TempDir::new("ex-export-bundle");
        let out = dir.0.join("out");
        run(args(&[sample_bundle(&dir).as_os_str(), out.as_os_str()])).unwrap();
        assert!(out.join("icon.png").exists());
    }

    #[test]
    fn exports_the_sprites_as_pngs() {
        let dir = TempDir::new("ex-export");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        let png = image::open(out.join("icon.png")).unwrap().to_rgba8();
        assert_eq!((png.width(), png.height()), (2, 1));
        // The sprite is the texture's bottom-left 2x1: the first two stored pixels, as stored.
        assert_eq!(png.into_raw(), (0..8).collect::<Vec<u8>>());
        // A prefix nothing matches exports nothing, and is not a failure.
        let none = dir.0.join("none");
        run(args(&[
            file.as_os_str(),
            none.as_os_str(),
            OsStr::new("zz"),
        ]))
        .unwrap();
        assert_eq!(std::fs::read_dir(&none).unwrap().count(), 0);
        // Again into the same folder: the file there is kept, and the run fails.
        assert!(run(args(&[file.as_os_str(), out.as_os_str()])).is_err());
    }

    #[test]
    fn a_missing_input_makes_no_folder() {
        let dir = TempDir::new("ex-export-missing");
        let out = dir.0.join("out");
        let missing = dir.0.join("missing.assets");
        assert!(run(args(&[missing.as_os_str(), out.as_os_str()])).is_err());
        assert!(!out.exists());
        assert_eq!(
            run(args(&[missing.as_os_str()])).unwrap_err().to_string(),
            USAGE
        );
    }
}

/// Added by the mutation review: what `export` writes, names and prints.
#[cfg(test)]
mod output {
    use super::*;
    use common::capture;
    use common::fixture::{args, sample, TempDir, SPRITE_ID};
    use common::more_fixtures::{self, broken, png, sprite_on, TEXTURE};
    use std::ffi::OsStr;

    /// The file names in `dir`, sorted.
    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn the_png_holds_the_sprites_own_pixels() {
        let dir = TempDir::new("mk-export-px");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        let _ = capture::out();
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        assert_eq!(capture::out(), ["exported 1, failed 0"]);
        let mut assets = unity_bundle_assets::Assets::open(&file).unwrap();
        let list = assets.sprites(|_| true);
        let want = assets.export(&list.sprites[0]).unwrap();
        assert_eq!(
            png(&out.join("icon.png")),
            (want.width(), want.height(), want.rgba().to_vec())
        );
    }

    #[test]
    fn any_prefix_selects() {
        let dir = TempDir::new("mk-export-prefix");
        let file = sample(&dir, false);
        let out = dir.0.join("out");
        run(args(&[
            file.as_os_str(),
            out.as_os_str(),
            OsStr::new("zz"),
            OsStr::new("ic"),
        ]))
        .unwrap();
        assert_eq!(names(&out), ["icon.png"]);
    }

    #[test]
    fn names_are_cleaned_and_kept_apart() {
        let dir = TempDir::new("mk-export-names");
        let file = more_fixtures::sprites(
            &dir,
            vec![
                (SPRITE_ID, sprite_on("icon", TEXTURE, 0, 0b10)),
                (2, sprite_on("Icon", TEXTURE, 0, 0b10)),
                (3, sprite_on("../evil", TEXTURE, 0, 0b10)),
            ],
        );
        let out = dir.0.join("out");
        run(args(&[file.as_os_str(), out.as_os_str()])).unwrap();
        assert_eq!(names(&out), [".._evil.png", "Icon_2.png", "icon.png"]);
        assert!(!dir.0.join("evil.png").exists());
    }

    #[test]
    fn a_sprite_it_cannot_read_is_counted_and_reported() {
        let dir = TempDir::new("mk-export-broken");
        let file = more_fixtures::sprites(
            &dir,
            vec![
                (SPRITE_ID, sprite_on("icon", TEXTURE, 0, 0b10)),
                (30, broken()),
            ],
        );
        let out = dir.0.join("out");
        let (_, _) = (capture::out(), capture::err());
        let e = run(args(&[file.as_os_str(), out.as_os_str()])).unwrap_err();
        assert_eq!(e.to_string(), "1 not exported");
        assert_eq!(capture::out(), ["exported 1, failed 1"]);
        let err = capture::err();
        assert_eq!(err.len(), 1, "{err:?}");
        assert!(err[0].starts_with("? (30): "), "{err:?}");
    }

    #[test]
    fn a_summary_that_cannot_be_written_is_an_error() {
        let dir = TempDir::new("mk-export-pipe");
        let file = sample(&dir, false);
        capture::fail_out(true);
        let e = run(args(&[file.as_os_str(), dir.0.join("out").as_os_str()]));
        capture::fail_out(false);
        let e = e.unwrap_err();
        assert_eq!(
            e.downcast_ref::<std::io::Error>().map(std::io::Error::kind),
            Some(std::io::ErrorKind::BrokenPipe)
        );
    }
}
