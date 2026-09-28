//! Read every sprite and count them by texture format, packing, and atlas use:
//! `cargo run --example survey -- <file>`.

#[macro_use]
mod common;

use std::collections::BTreeMap;
use unity_bundle_assets::{Assets, Error, Settings};

fn main() -> std::process::ExitCode {
    common::finish(run())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("usage: survey <file>")?;
    let assets = Assets::open(&path)?;
    let list = assets.sprites(|_| true);
    let mut printed = 0;
    for s in &list.skipped {
        common::report(&mut printed, || {
            format!(
                "sprite {}: {}",
                s.path_id,
                common::printable(&s.error.to_string())
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
            settings.map_or_else(|| "?".into(), |s| format!("{:?}", s.rotation())),
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
        Error::LimitExceeded { kind, .. } => format!("limit {kind:?}"),
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
