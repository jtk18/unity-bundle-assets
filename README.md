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
        let image = assets.export(sprite)?; // RGBA8, top row first
    }
    for skipped in &list.skipped {
        eprintln!("sprite {} not read: {}", skipped.path_id, skipped.error);
    }
    Ok(())
}
```

`sprites` returns sprites ordered by the texture that holds them, so `export` decodes each
texture (a 4096x4096 atlas is 64 MB decoded) once. To use several threads, group sprites by
`texture_id`, then per group call `decode_texture` once and `cut` for each sprite; both take
`&self`, and `Assets` is `Send + Sync`.

`Assets::open` also takes an asset bundle holding one serialized file, which is the usual case,
and textures can be listed and decoded directly:

```rust,no_run
fn main() -> Result<(), unity_bundle_assets::Error> {
    let assets = unity_bundle_assets::Assets::open("assetbundles/characters")?;
    for texture in assets.textures(|_| true) {
        let image = assets.decode_texture(texture.path_id)?; // RGBA8, top row first
    }
    Ok(())
}
```

For a bundle holding several serialized files (scene bundles), parse it with `Bundle::parse`
and open each with `Assets::from_bundle`.

## What it handles

- Serialized file format versions 17 to 22, little- or big-endian. Object layouts are
  hard-coded rather than read from type trees, because player builds usually strip them.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
  A file whose engine version was stripped takes its bundle's.
- `Texture2D` from Unity 5.5 through 6000.4, final and beta builds, with pixels inline or
  streamed from a `.resS` file beside the serialized file or inside the same bundle. Fields are
  gated by engine release, checked against the per-release type trees UnityPy ships, which
  run to the 6000.4 betas.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 through 6000.4. Every object read must end
  where its last field does, so a layout misread is an error rather than wrong numbers.
- Packing rotation and flips, and tight packing (pixels outside the sprite's mesh are cleared,
  colour and alpha).
- Texture formats Alpha8, RGB24, RGBA32, ARGB32, BGRA32, DXT1 (BC1), DXT5 (BC3).

Exports follow AssetStudio where references disagree: the sprite's texture rectangle, not
padded out to its full rect; Alpha8 as white with the stored alpha (UnityPy gives black);
`Rotate90` packing undone counter-clockwise (UnityPy turns the other way, and no real sample
here settles it). A tight sprite's mask keeps a pixel when the mesh covers any of four points
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
  PVRTC, RGB565, ARGB4444, RGBA4444, R8, R16, RG16 and the half- and float-precision formats.
- Textures from files built for consoles (PlayStation, Xbox, Switch, Wii U, 3DS), whose GPU
  tiling this crate does not undo. This is a list of known console platforms: a file for a
  console Unity adds later would be decoded as if untiled.
- Sprites whose alpha is in a separate texture, sprites in a downscaled atlas, and tight-packed
  sprites whose mesh this crate cannot read.
- Textures and atlases that live in another serialized file, and sprites that find their atlas
  only by tag at run time (no atlas reference in the file).
- Textures whose smaller mip levels were stripped from the build.

## Untrusted input

Files are treated as hostile:

- Counts and lengths are checked against the bytes present, at each record's real minimum
  size, before they size an allocation. Every string read (names, paths, versions) is at most
  4 KiB, so nothing the crate holds or quotes in an error grows with a lying length.
- One blob cannot be decoded many times over: objects may not overlap or share an ID, bundle
  entries and a sprite's sub-meshes may not overlap, and a range of stream data may be read by
  one texture only. Ranges are compared by the bytes they reach, not by how the path to them
  is spelled; a texture may not stream from a serialized file.
- A file is checked by its header before the rest is read, and a bundle is read only up to
  the size its header declares.
- Sizes the data alone cannot bound are held to `Limits`, which a caller can lower: file size
  (2 GiB); decompressed bundle size (1 GiB), which also counts the parsed directory and LZMA's
  working memory (its tables for every block, and its dictionary); objects per file (4M);
  pixels per texture (16384 x 16384); triangles per sprite (65,536) and per `sprites` call
  (4M); mask work per sprite (2^29); and total work (2^34): every pixel decoded, masked and
  copied. The total is shared by every `Assets` opened from one `Bundle` (two `Assets::open`
  calls on one path are two totals). Work is reserved before it starts; a request refused
  then, or data refused before it is read, costs nothing, and work once started is kept.
- `Assets` enforces all of that. Used directly, `Bundle` and `SerializedFile` apply their own
  limits, `Texture2D` refuses data too short for its size, and `decode::decode` applies none;
  a caller using them counts its own work.
- The limits bound work, not peak memory. At the default 16384 x 16384, decoding one texture
  can hold 1 to 2 GiB (the stored pixels and the RGBA result), and exporting a sprite from it
  up to 3 GiB; each thread decoding and cutting in parallel holds about that much. LZMA can
  expand a 150 KB file to the full 1 GiB of decompressed data. A bundle is decompressed whole
  when it is opened, and one parsed from bytes briefly holds both the file and its
  decompressed copy. Lower `max_texture_pixels` and `max_decompressed` where that matters.
- Streamed pixels are read only from the same bundle, or from a `.resS` / `.resource` file
  directly beside the asset file: one plain file name, a regular file, not a symbolic link,
  and not named like a Windows device however spelled (`CON`, `nul .resS`, `COM1`, ...) or an
  alternate data stream (`:`). Files are opened without blocking and without taking a
  controlling terminal (on Unix targets whose flag values the crate knows; elsewhere a FIFO
  put in place between check and open can block it). On Unix the file opened must be the one
  checked and have no other hard links. On Windows neither a file swapped in between check and open nor a hard link is
  detected. The folder itself is looked up by path for each texture, so whoever can rename
  folders on the way to it while a program runs can point the next read elsewhere.
- Strings from the file are quoted in error messages, so printing an error cannot send control
  sequences to a terminal. Strings the API returns (names, paths) are the file's raw text:
  escape them before printing.
- `ObjectInfo` and `Entry` values are not tied to the file they came from: given to another
  file's readers they read that file's bytes at their offsets.

Malformed input is meant to give an error rather than a panic. `tests/fuzz.rs` checks that
over mutated files (set `UBA_FUZZ_ITERS` to run longer); it is evidence, not proof.

## Tested against

- A Unity 2022.3 macOS player build: all 9,436 sprites of its 754 MB `sharedassets0.assets`
  export, and 1,756 of its 1,795 textures decode (the rest are 27 empty font textures and 12
  BC7). Against UnityPy, 8,878 sprites are pixel-identical, 557 differ only in the colour of
  fully transparent pixels (masking clears it; UnityPy keeps the texel), and one differs in 2
  pixels at a mask edge. Exporting every sprite and decoding every texture of that file takes
  4.6 s and 0.96 GB of memory on an Apple silicon Mac. In its `resources.assets`, 174 of 176
  sprites export (two use a texture in another file); 164 match UnityPy up to transparent
  colour and 10 differ at mask edges, in 1 to 136 pixels each. Across all 145 of the build's
  asset files and bundles, 5,983 textures decode; the 75 that do not are 43 empty, 21 BC7, 10
  RGBAFloat and one RHalf.
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

1.83 for the library, set by its `crc` dependency and by its own `const fn`s that take
`&mut self`. Checked with Clippy's `incompatible_msrv` and by building on 1.85, the oldest
toolchain at hand; not yet built on 1.83 itself. The tests and examples use the `image`
crate, which needs 1.88.

## License

MIT. The bundle reader follows UnityPy (MIT); see NOTICE.
