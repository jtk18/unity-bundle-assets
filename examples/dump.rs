//! Hex-dump one object: `cargo run --example dump -- <file-or-bundle> <path-id> [max-bytes]`.

mod common;

fn main() -> std::process::ExitCode {
    common::finish(run())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: dump <file> <path-id> [max-bytes]")?;
    let id: i64 = common::text_arg(&mut args, "the path id")?
        .ok_or("usage: dump <file> <path-id> [max-bytes]")?
        .parse()?;
    let max: usize = common::text_arg(&mut args, "the byte count")?
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(512);
    let assets = unity_bundle_assets::Assets::open(&path)?;
    let file = assets.file();
    let object = file.object(id).ok_or("no such object")?;
    println!("class {} size {}", object.class_id(), object.size());
    let bytes = file.bytes(object).ok_or("object outside the file")?;
    for (i, chunk) in bytes[..object.size().min(max)].chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let text: String = chunk
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        println!("{:06x}  {:<48} {text}", i * 16, hex.join(" "));
    }
    Ok(())
}
