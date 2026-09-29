//! Hex-dump one object: `cargo run --example dump -- <file-or-bundle> <path-id> [max-bytes]`.

#[macro_use]
mod common;

/// How to call it; the README's line for this example must match.
const USAGE: &str = "usage: dump <file-or-bundle> <path-id> [max-bytes]";

fn main() -> std::process::ExitCode {
    common::finish(run(std::env::args_os().skip(1)))
}

fn run(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = args.next().ok_or(USAGE)?;
    let id: i64 = common::text_arg(&mut args, "the path id")?
        .ok_or(USAGE)?
        .parse()
        .map_err(|e| format!("the path id: {e}"))?;
    let max: usize = common::text_arg(&mut args, "the byte count")?
        .map(|s| s.parse().map_err(|e| format!("the byte count: {e}")))
        .transpose()?
        .unwrap_or(512);
    let assets = unity_bundle_assets::Assets::open(&path)?;
    let file = assets.file();
    let object = file
        .object(id)
        .ok_or_else(|| format!("no object with path id {id}"))?;
    outln!("class {} size {}", object.class_id(), object.size());
    let bytes = file.bytes(object).ok_or("object outside the file")?;
    for (i, chunk) in bytes[..object.size().min(max)].chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let text: String = chunk
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        outln!("{:06x}  {:<48} {text}", i * 16, hex.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::fixture::{args, sample, TempDir, TEXTURE};
    use std::ffi::OsStr;

    #[test]
    fn dumps_an_object_and_names_a_missing_one() {
        let dir = TempDir::new("ex-dump");
        let file = sample(&dir, false);
        let id = TEXTURE.to_string();
        run(args(&[file.as_os_str(), OsStr::new(&id)])).unwrap();
        run(args(&[file.as_os_str(), OsStr::new(&id), OsStr::new("16")])).unwrap();
        let e = run(args(&[file.as_os_str(), OsStr::new("999")])).unwrap_err();
        assert_eq!(e.to_string(), "no object with path id 999");
        assert_eq!(
            run(args(&[file.as_os_str()])).unwrap_err().to_string(),
            USAGE
        );
    }
}
