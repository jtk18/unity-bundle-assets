//! Hex-dump one object: `cargo run --example dump -- <file> <path-id> [max-bytes]`.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: dump <file> <path-id> [max-bytes]")?;
    let id: i64 = args.next().ok_or("path id")?.parse()?;
    let max: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(512);
    let file = unity_sprites::SerializedFile::open(path.as_ref())?;
    let object = file.object(id).ok_or("no such object")?;
    println!("class {} size {}", object.class_id, object.size);
    for (i, chunk) in file.bytes(object)[..object.size.min(max)]
        .chunks(16)
        .enumerate()
    {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let text: String = chunk
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        println!("{:06x}  {:<48} {text}", i * 16, hex.join(" "));
    }
    Ok(())
}
