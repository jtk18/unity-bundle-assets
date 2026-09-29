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
    use common::fixture::{args, sample, sample_bundle, TempDir, TEXTURE};
    use std::ffi::OsStr;

    #[test]
    fn dumps_from_a_bundle() {
        let dir = TempDir::new("ex-dump-bundle");
        let id = TEXTURE.to_string();
        run(args(&[sample_bundle(&dir).as_os_str(), OsStr::new(&id)])).unwrap();
    }

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

/// Added by the mutation review: the original tests ran `dump` but never read its output.
#[cfg(test)]
mod output {
    use super::*;
    use common::capture;
    use common::fixture::{args, sample, TempDir, TEXTURE};
    use std::ffi::OsStr;

    /// The object's bytes, read through the library.
    fn object_bytes(file: &std::path::Path) -> Vec<u8> {
        let assets = unity_bundle_assets::Assets::open(file).unwrap();
        let o = assets.file().object(TEXTURE).unwrap();
        assets.file().bytes(o).unwrap()[..o.size()].to_vec()
    }

    /// Checks each dump line against `bytes` by reading it back.
    fn check(lines: &[String], bytes: &[u8]) {
        assert_eq!(lines.len(), bytes.len().div_ceil(16), "{lines:?}");
        for (i, (line, chunk)) in lines.iter().zip(bytes.chunks(16)).enumerate() {
            assert_eq!(&line[..8], format!("{:06x}  ", i * 16), "{line}");
            let hex = &line[8..8 + 48];
            let got: Vec<u8> = hex
                .split_whitespace()
                .map(|h| {
                    assert_eq!(h, h.to_ascii_lowercase(), "{line}");
                    u8::from_str_radix(h, 16).unwrap()
                })
                .collect();
            assert_eq!(got, chunk, "{line}");
            let text: String = chunk
                .iter()
                .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
                .collect();
            assert_eq!(&line[8 + 48 + 1..], text, "{line}");
        }
    }

    #[test]
    fn dumps_exactly_the_bytes_asked_for() {
        let dir = TempDir::new("mk-dump");
        let file = sample(&dir, false);
        let bytes = object_bytes(&file);
        assert!(bytes.len() > 20 && bytes.len() <= 512, "{}", bytes.len());
        assert!(
            bytes.iter().any(u8::is_ascii_graphic) && bytes.iter().any(|b| !b.is_ascii_graphic())
        );
        let id = TEXTURE.to_string();
        let _ = capture::out();
        run(args(&[file.as_os_str(), OsStr::new(&id)])).unwrap();
        let out = capture::out();
        assert_eq!(out[0], format!("class 28 size {}", bytes.len()));
        check(&out[1..], &bytes);
        run(args(&[file.as_os_str(), OsStr::new(&id), OsStr::new("20")])).unwrap();
        let out = capture::out();
        check(&out[1..], &bytes[..20]);
    }

    #[test]
    fn says_which_argument_is_wrong() {
        let dir = TempDir::new("mk-dump-args");
        let file = sample(&dir, false);
        let e = run(args(&[file.as_os_str(), OsStr::new("x")])).unwrap_err();
        assert!(e.to_string().starts_with("the path id: "), "{e}");
        let e = run(args(&[file.as_os_str(), OsStr::new("10"), OsStr::new("x")])).unwrap_err();
        assert!(e.to_string().starts_with("the byte count: "), "{e}");
    }
}
