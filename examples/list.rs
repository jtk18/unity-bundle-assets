//! List a serialized file's objects: `cargo run --example list -- <file> [class-id]`.

use std::collections::BTreeMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: list <file> [class-id]")?;
    let filter: Option<i32> = args.next().map(|s| s.parse()).transpose()?;
    let file = unity_sprites::SerializedFile::open(path.as_ref())?;
    println!(
        "format {} unity {} platform {} type trees {} externals {:?}",
        file.version,
        file.unity_version,
        file.target_platform,
        file.has_type_trees,
        file.externals.iter().map(|e| &e.path).collect::<Vec<_>>()
    );
    let mut counts = BTreeMap::new();
    for o in &file.objects {
        *counts.entry(o.class_id).or_insert(0) += 1;
        if Some(o.class_id) == filter {
            println!(
                "{:>20} {:>9} {}",
                o.path_id,
                o.size,
                file.name(o).unwrap_or_default()
            );
        }
    }
    println!("{counts:?}");
    Ok(())
}
