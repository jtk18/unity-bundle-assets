//! Read every sprite and count them by texture format, packing, and atlas use:
//! `cargo run --example survey -- <file-or-bundle>`.

#[macro_use]
mod common;

use std::collections::BTreeMap;
use unity_bundle_assets::{Assets, Error, Settings};

/// How to call it; the README's line for this example must match.
const USAGE: &str = "usage: survey <file-or-bundle>";

fn main() -> std::process::ExitCode {
    common::finish(run(std::env::args_os().skip(1)))
}

fn run(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.next().ok_or(USAGE)?;
    let assets = Assets::open(&path)?;
    let list = assets.sprites(|_| true);
    let mut printed = 0;
    for s in &list.skipped {
        common::report(&mut printed, || {
            format!(
                "sprite {}: {}",
                s.path_id,
                common::printable_error(&s.error.to_string())
            )
        });
    }
    let mut formats = BTreeMap::new();
    let mut by = BTreeMap::new();
    for sprite in &list.sprites {
        let placement = assets.placement(sprite);
        let tex = match &placement {
            // Grouped by kind, not by message: each sprite's message names the sprite.
            Err(e) => format!("error: {}", kind(e)),
            Ok(p) if p.texture.file_id != 0 => format!("external {}", p.texture.file_id),
            Ok(p) => {
                let t = formats.entry(p.texture.path_id).or_insert_with(|| {
                    assets
                        .texture(p.texture.path_id)
                        .map(|t| t.format)
                        .map_err(|e| kind(&e))
                });
                match t {
                    Ok(format) => format!("format {format}"),
                    Err(kind) => format!("texture error: {kind}"),
                }
            }
        };
        // Packing is unknown when the placement is: counted apart, not as defaults.
        let settings = placement.as_ref().ok().map(|p| p.settings);
        *by.entry((
            tex,
            settings.map(Settings::tight),
            // Rotation counts only for packed sprites, as `Assets::cut` reads it.
            settings.map_or_else(
                || "?".into(),
                |s| {
                    if s.packed() {
                        s.rotation()
                            .map_or_else(|| "?".into(), |r| format!("{r:?}"))
                    } else {
                        "Unrotated".into()
                    }
                },
            ),
            !sprite.atlas.is_null(),
        ))
        .or_insert(0) += 1;
    }
    outln!(
        "sprites read {}, skipped {}",
        list.sprites.len(),
        list.skipped.len()
    );
    let groups = by.len();
    for (k, n) in by.into_iter().take(common::MAX_REPORTED) {
        let (texture, tight, rotation, atlas) = k;
        let tight = match tight {
            Some(true) => "tight",
            Some(false) => "rect",
            None => "?",
        };
        outln!("{n:>6} {texture} {tight} {rotation} atlas={atlas}");
    }
    if groups > common::MAX_REPORTED {
        outln!("({} more groups not shown)", groups - common::MAX_REPORTED);
    }
    Ok(())
}

/// What kind of error this is, without the names and numbers in its message.
fn kind(e: &Error) -> String {
    match e.root() {
        Error::Truncated(_) => "truncated".into(),
        Error::BadLength { .. } => "bad length".into(),
        Error::NotUnity(_) => "not a Unity file".into(),
        Error::Unsupported(_) => "unsupported".into(),
        Error::UnsupportedTextureFormat { format, .. } => format!("texture format {format}"),
        Error::Encrypted => "encrypted".into(),
        Error::LimitExceeded { kind, .. } => format!("limit: {kind}"),
        Error::NotFound(_) => "not found".into(),
        Error::WrongClass { .. } => "wrong class".into(),
        Error::EmptyTexture(_) => "empty texture".into(),
        Error::Invalid(_) => "invalid".into(),
        Error::InvalidArgument(_) => "invalid argument".into(),
        Error::OutOfMemory { .. } => "out of memory".into(),
        Error::Io { error, .. } => format!("io {:?}", error.kind()),
        _ => "other".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::fixture::{args, sample, sample_bundle, TempDir};

    #[test]
    fn surveys_a_bundle() {
        let dir = TempDir::new("ex-survey-bundle");
        run(args(&[sample_bundle(&dir).as_os_str()])).unwrap();
    }

    #[test]
    fn surveys_a_file() {
        let dir = TempDir::new("ex-survey");
        let file = sample(&dir, true);
        run(args(&[file.as_os_str()])).unwrap();
        assert_eq!(run(args(&[])).unwrap_err().to_string(), USAGE);
    }
}

/// Added by the mutation review: what `survey` counts and prints.
#[cfg(test)]
mod output {
    use super::*;
    use common::capture;
    use common::fixture::{args, TempDir, SPRITE_ID};
    use common::more_fixtures::{self, broken, sprite_on, TEXTURE};

    #[test]
    fn groups_sprites_by_texture_packing_and_atlas() {
        let dir = TempDir::new("mk-survey");
        let file = more_fixtures::sprites(
            &dir,
            vec![
                (SPRITE_ID, sprite_on("icon", TEXTURE, 0, 0b10)),
                // Packed, rectangle, flipped horizontally.
                (2, sprite_on("packed", TEXTURE, 0, 0b111)),
                // Packed, tight, unrotated.
                (3, sprite_on("tight", TEXTURE, 0, 0b01)),
                // In an atlas the file does not hold.
                (4, sprite_on("atlased", TEXTURE, 5, 0b10)),
                // On a texture the file does not hold.
                (5, sprite_on("orphan", 99, 0, 0b10)),
                (30, broken()),
            ],
        );
        let (_, _) = (capture::out(), capture::err());
        run(args(&[file.as_os_str()])).unwrap();
        assert_eq!(
            capture::out(),
            [
                "sprites read 5, skipped 1",
                "     1 format 4 rect FlipHorizontal atlas=false",
                "     1 format 4 rect Unrotated atlas=false",
                "     1 format 4 rect Unrotated atlas=true",
                "     1 format 4 tight Unrotated atlas=false",
                "     1 texture error: not found rect Unrotated atlas=false",
            ]
        );
        let err = capture::err();
        assert_eq!(err.len(), 1, "{err:?}");
        assert!(err[0].starts_with("sprite 30: "), "{err:?}");
    }

    #[test]
    fn a_file_it_cannot_open_is_an_error() {
        let dir = TempDir::new("mk-survey-missing");
        assert!(run(args(&[dir.0.join("missing").as_os_str()])).is_err());
    }
}
