# unity-bundle-assets

Read Unity serialized asset files (`*.assets`, `level*`) and asset bundles, and export their
sprites and textures as RGBA images, in pure Rust. No .NET, no native libraries, no external
tools, and no `unsafe` in this crate.

```rust,no_run
use unity_bundle_assets::Assets;

fn main() -> Result<(), unity_bundle_assets::Error> {
    let mut assets = Assets::open("Game_Data/sharedassets0.assets")?;
    let list = assets.sprites(|name| name.starts_with("Icon_"));
    for sprite in &list.sprites {
        match assets.export(sprite) {
            // Names come from the file: `{:?}` escapes anything a terminal would act on.
            Ok(image) => println!("{:?}: {}x{}", sprite.name, image.width(), image.height()),
            Err(e) => eprintln!("sprite {} not exported: {e}", sprite.path_id),
        }
    }
    for skipped in &list.skipped {
        eprintln!("sprite {} not read: {}", skipped.path_id, skipped.error);
    }
    Ok(())
}
```

`sprites` returns sprites ordered by the texture that holds them, so `export` decodes each
texture (a 4096x4096 atlas is 64 MiB decoded) once. To use several threads, group sprites by
`texture_id`, then per group call `decode_texture` once and `cut` for each sprite; both take
`&self`, and `Assets` is `Send + Sync`.

`Assets::open` also takes an asset bundle holding one serialized file, which is the usual case,
and textures can be listed and decoded directly:

```rust,no_run
fn main() -> Result<(), unity_bundle_assets::Error> {
    let assets = unity_bundle_assets::Assets::open("assetbundles/characters")?;
    for texture in assets.textures(|_| true) {
        // RGBA8, top row first; a texture in a format this crate does not decode is skipped.
        if let Ok(image) = assets.decode_texture(texture.path_id) {
            println!("{}: {} bytes", texture.path_id, image.rgba().len());
        }
    }
    Ok(())
}
```

For a bundle holding several serialized files (scene bundles), open it with `Bundle::open`
and open each with `Assets::from_bundle`.

## What it handles

- Serialized file format versions 17 to 22, little- or big-endian. Object layouts are
  hard-coded rather than read from type trees, because player builds usually strip them.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
  A file whose engine version was stripped takes its bundle's.
- `Texture2D` from Unity 5.5 through 6000.4 (final, patch, China and beta builds), with pixels
  inline or streamed from a `.resS` file beside the serialized file or inside the same bundle.
  Fields are gated by engine release, checked against the per-release type trees UnityPy
  ships; those run to the 6000.4 betas, so 6000.4 finals are read with the betas' layout,
  unchecked.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 through 6000.4, likewise. Every object read
  must end exactly where its last field does, so a layout misread is an error rather than
  wrong numbers.
- Packing rotation and flips, and tight packing (pixels outside the sprite's mesh are cleared,
  colour and alpha).
- Texture formats Alpha8, ARGB4444, RGBA4444, RGB565, RGB24, RGBA32, ARGB32, BGRA32, DXT1
  (BC1), DXT5 (BC3).

Exports follow AssetStudio where references disagree: the sprite's texture rectangle, not
padded out to its full rect; Alpha8 as white with the stored alpha (UnityPy gives black);
`Rotate90` packing undone counter-clockwise (UnityPy turns the other way, and no real sample
here settles it); the mask's pixel grid starts at the texture rectangle's own origin, not at
the whole pixel the cut-out starts from, so it can sit a fraction of a pixel off it; and a
triangle with no area covers nothing (Pillow, under UnityPy, draws it as a line). A tight
sprite's mask keeps a pixel when the mesh covers any of four points
at its quarter positions. Measured against UnityPy's polygon fill over two real files, that
rule disagreed on 324 pixels; testing the pixel centre alone disagreed on 349, any overlap on
344.

## What it doesn't handle (yet)

Each of these gives an error naming it:

- Encrypted bundles, the older `UnityWeb` / `UnityRaw` containers, and bundles whose textures
  stream from a different bundle.
- Engine releases before 5.5 (textures) or 2019.1 (sprites), after 6000.4, alpha builds, and
  files whose engine version was stripped outside a bundle.
- Every texture format not listed above, among them crunch-compressed, BC4-7, ETC, EAC, ASTC,
  PVRTC, R8, R16, RG16 and the half- and float-precision formats.
- Textures from files built for consoles (PlayStation, Xbox, Switch, Wii U, 3DS), whose GPU
  tiling this crate does not undo. This is a list of known console platforms: a file for a
  console Unity adds later would be decoded as if untiled.
- Sprites whose alpha is in a separate texture, sprites in a downscaled atlas, and tight-packed
  sprites whose mesh this crate cannot read.
- Textures and atlases that live in another serialized file, and sprites that find their atlas
  only by tag at run time (no atlas reference in the file).
- Textures whose smaller mip levels were stripped from the build, and sprites from Unity 2018.
- Files an Android build split into `.split0`, `.split1`, ... parts (join them first).

## Untrusted input

Files are treated as hostile:

- Counts and lengths are checked against the bytes present, at each record's real minimum
  size, before they size an allocation. Every string kept or shown is at most 4 KiB of the
  file; one that is only stepped over is bounded by its object. Versions and the paths used to
  find data (bundle entries, texture streams) must be UTF-8, as Unity writes them; a name, or
  a dependency's path (which is only reported), that is not has its bad bytes shown as U+FFFD,
  and is cut to 4 KiB as text. Error messages quote at most 64 characters of any name or path
  (escaped, so a few hundred bytes at most).
- One blob cannot be decoded many times over: objects may not overlap or share an ID, bundle
  entries and a sprite's sub-meshes may not overlap, and a range of stream data may be read by
  one texture only. Ranges are compared by the bytes they reach (on Unix, the file's device
  and inode; elsewhere its lower-cased name; stream names must be ASCII everywhere), not by
  how the path to them is spelled; in a bundle no texture may stream from a serialized entry,
  and a file opened with `Assets::open` may not stream from itself (a texture read through
  `Assets::from_bytes` or `Assets::from_serialized` is not checked for that: they do not know
  which file their bytes came from). Claims last as long as the `Assets` (or the `Bundle`), so
  separate `Assets::open` calls on files that share a stream file each claim its ranges
  afresh.
- A file is checked by its header before the rest is read. A bundle must declare a size
  between its header's and its data's, and is read only that far; a serialized file is read
  as far as its header says it runs. A file that states a large size and holds nothing (a
  sparse file, say) is still read that far: up to `max_file_size`. Stream files are read by
  range and are not held to `max_file_size` (real ones pass 2 GiB); what is decoded from them
  is held to the work limits.
- Sizes the data alone cannot bound are held to `Limits`, which a caller can lower: file size
  (2 GiB); decompressed bundle size (1 GiB), which also counts the parsed directory and LZMA's
  working memory (its tables for every block, directory included, and its dictionary); objects
  (4M, across all the files of a bundle); pixels per texture (16384 x 16384); triangles per
  sprite (65,536) and per `sprites` call (4M); mask work per sprite (2^29) and total work
  (2^34). Work is counted in units of about one pixel's: a unit for each pixel decoded (whole
  4x4 blocks for DXT) or copied (three for a quarter-turned sprite, which reads the texture
  down its columns) and each column tested for a mask; 64 for each decode or cut asked for,
  refused or not, and a unit for each byte of a texture's name and stream path past 64; 8192
  more each time a stream file is looked for and 192 more for a stream in the same bundle,
  whether or not its range is then granted; and 16 for each mesh triangle and each row a
  triangle crosses. Mesh vertices must lie within 65,536 pixels of the image's corner, after
  pivot and offset. A unit costs about 2 ns on an Apple silicon Mac, so the total is under a
  minute of CPU. It is shared by every `Assets` opened from one `Bundle` (two `Assets::open`
  calls on one path are two totals), and it is never given back: a long-running program that
  decodes the same textures again and again should raise it. Work is reserved before it
  starts, and kept once reserved: a refusal keeps the steps already reserved (the call, a long
  name, a stream file opened, and a mask's triangles and rows, reserved all at once before any
  is worked out, so a mask refused partway still pays for all of them) and is not charged for
  pixels or columns it never touched. Pixels reserved stay charged if reading or allocating
  them then fails. Once the total is spent, every decode and cut is refused before it reads
  anything. Which of several threads' requests are refused under the limit depends on their
  timing, and so, when two textures name the same stream range, does which one decodes it.
  Opening files, listing their contents and reading objects (`Assets::texture`,
  `Assets::placement`) are not counted: decompressing LZMA runs at up to about 55 ns a byte
  (incompressible data), so a bundle that decompresses to the default 1 GiB can take about a
  minute of CPU to open, before any work limit applies; lower `max_decompressed` for bundles
  from strangers. Images one pixel wide cost up to about 3.5 ns a unit, and refusals whose
  message quotes a long non-ASCII name up to about 15 ns (a budget spent wholly on those takes
  some four minutes); many tiny LZMA blocks decompress at up to about 150 ns a byte, but the
  directory charge caps them at some 65,000 blocks, a fraction of a second; and every figure
  here is CPU on a local disk: on a network share each stream file opened can take
  milliseconds.
- `Assets` enforces all of that. Used directly, `Bundle` and `SerializedFile` apply their own
  limits, `Texture2D` refuses data too short for its size but claims no ranges (texture after
  texture may read the same bytes), and `decode::decode` applies none; a caller using them
  counts its own work.
- The limits bound work, not peak memory. Opening a file holds the file (up to
  `max_file_size`) or a bundle's decompressed data (up to `max_decompressed`); a bundle is
  decompressed whole when opened, and holds its compressed copy too until it is parsed. The
  object table takes about 100 bytes an object (about 400 MiB at `max_objects`), and each
  opening of a bundle's file builds its own. On top of that, at the default 16384 x 16384,
  decoding one texture holds up to about 1 GiB more for the RGBA result; a streamed texture's
  stored pixels are widened to it in place, except DXT's, which are held beside it while they
  are decoded: up to 1.25 GiB in all (DXT5). Exporting a sprite holds up to about 2.25 GiB
  more (the decoded texture, the sprite and its mask); each thread decoding and cutting in
  parallel holds about that much. Names are held as text, at most 4 KiB each: every sprite
  `Assets::sprites` reads and every texture `Assets::textures` lists keeps its name, and each
  sprite it could not read keeps an entry of some 220 bytes besides its name, up to
  `max_objects`; so a file of tiny broken sprites can make a list some eight to ten times its
  size. Each sprite's mesh is held at 24 bytes a triangle, up to about 96 MiB for a list at
  the default `max_total_triangles`. Each stream range decoded is remembered, some 100 bytes,
  for the life of the `Assets`, and so is each atlas read (about 1.5 times its bytes) or
  refused (some 430 bytes). A file's dependency list is held as text too, some 40 bytes a
  dependency besides its path, and its dependencies count with its objects toward
  `max_objects`. Together, a small LZMA bundle of long names can still make opening and
  listing hold a few times `max_decompressed`: lower it for bundles from strangers. LZMA can
  expand a 150 KB file to the full 1 GiB of decompressed data, and 40 KB of a compressed
  bundle can make a 1 GiB texture. Allocations sized by the file fail as `Error::OutOfMemory`
  where the crate makes them, but that is best effort: LZMA's own buffers, the object table
  and other small growth abort on failure as usual, and an operating system that overcommits
  may never refuse. For files from strangers, lower `max_decompressed`, `max_texture_pixels`
  and `max_total_work`.
- Streamed pixels are read only from the same bundle, or from a `.resS` / `.resource` file
  directly beside the asset file: one plain file name, a regular file, not a symbolic link,
  and not named like a Windows device however spelled (`CON`, `nul .resS`, `COM1`, ...) or an
  alternate data stream (`:`). Files are opened without blocking and without taking a
  controlling terminal (on Unix targets whose flag values the crate knows; elsewhere a FIFO
  put in place between check and open can block it). On Unix the file opened must be the one
  checked and have no other hard links. On Windows neither a file swapped in between check
  and open nor a hard link is detected. The folder itself is looked up by path for each
  texture, so whoever can rename folders on the way to it while a program runs can point the
  next read elsewhere.
- Strings from the file are quoted in error messages, so printing an error cannot send control
  sequences to a terminal. Strings the API returns (names, paths) are the file's raw text:
  escape them before printing.
- `ObjectInfo` and `Entry` values are not tied to the file they came from: given to another
  file's readers they read that file's bytes at their offsets. A stream path that names an
  entry by file name alone (`archive:/<other>/<name>`) finds the one entry of that name, or
  none when two entries share it.
- An error that `Assets::export` or an atlas passes on wraps its cause
  (`Error::TextureUnreadable`, `Error::AtlasUnreadable`); `Error::root` unwraps it, and
  `Error::io_error` finds an I/O error inside.

Malformed input is meant to give an error rather than a panic. `tests/fuzz.rs` checks that
over mutated files (set `UBA_FUZZ_ITERS` to run longer); it is evidence, not proof.

## Tested against

- A Unity 2022.3 macOS player build: all 9,436 sprites of its 754 MB `sharedassets0.assets`
  export, and 1,756 of its 1,795 textures decode (the rest are 27 empty font textures and 12
  BC7). Against UnityPy, 8,878 sprites are pixel-identical, 557 differ only in the colour of
  fully transparent pixels (masking clears it; UnityPy keeps the texel), and one differs in 2
  pixels at a mask edge. Exporting every sprite and decoding every texture of that file takes
  about 4.5 s and 1 GB of memory (0.95 to 1.07 GB peak across runs and harnesses) on an Apple
  silicon Mac. In its `resources.assets`, 174 of 176 sprites export (two use a texture in
  another file); 164 match UnityPy up to transparent colour and 10 differ at mask edges, in 1
  to 136 pixels each. Across all 148 Unity files of the build (57 serialized files, built-in
  resources included, and 91 bundles), 6,022 textures decode; the 76 that do not are 44
  empty, 21 BC7, 10 RGBAFloat and one RHalf. Its 34 ARGB4444 textures match UnityPy exactly.
- 737 asset bundles from a Unity 5.6.6, 5.6.7 and 2018.4 (.2, .11, .36) game and its mods: all
  open, and 15,193 textures export; the one failure is a dynamic font texture stored empty. On
  a sample of 171 bundles, 3,042 of the 3,056 textures UnityPy could decode are pixel-identical
  to its output, and the other 14 are the Alpha8 colour convention above, with identical alpha.
  Four of those bundles repacked as LZMA export byte-identical PNGs.
- The tests build serialized files and bundles byte by byte, since game files cannot ship with
  the crate: `Texture2D` in the layouts of fifteen engine releases from 5.6 to 6000.4, every
  metadata section, sprites with every packing rotation, mesh layout and mask case, atlases,
  big-endian files, every bundle container variant and flag, and hostile files for each limit.
  Real files cover only 5.6, 2018.4 and 2022.3; the version gates between and after rest on
  the type trees and on those built files, written from the same reading of them. No real
  file here has a rotated or flipped sprite.

The examples are the tools used to work out and check the layouts:

```sh
cargo run --example list -- <file> [class-id]           # objects by class, with names
cargo run --example dump -- <file> <path-id> [bytes]     # hex dump of one object
cargo run --example survey -- <file>                     # sprites by texture format and packing
cargo run --release --example export -- <file> <out-dir> [name-prefix...]
cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]
```

## Minimum Rust version

1.83 for the library, set by its own `const fn`s that take `&mut self` (and by recent releases
of its `crc` dependency). Checked with Clippy's `incompatible_msrv` and by building on 1.85,
the oldest toolchain at hand; not yet built on 1.83 itself. The tests and examples use the
`image` crate, which needs 1.88.

## License

MIT. The bundle reader follows UnityPy (MIT); see NOTICE.
