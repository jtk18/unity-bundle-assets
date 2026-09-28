# unity-bundle-assets

Read Unity serialized asset files (`*.assets`, `level*`) and asset bundles, and export their
sprites and textures as RGBA images, in pure Rust. No .NET, no native libraries, no external
tools, no `unsafe`.

```rust,ignore
use unity_bundle_assets::Assets;

let mut assets = Assets::open("Game_Data/sharedassets0.assets")?;
let list = assets.sprites(|name| name.starts_with("Icon_"));
for sprite in &list.sprites {
    let image = assets.export(sprite)?; // RGBA8, top row first
}
for skipped in &list.skipped {
    eprintln!("sprite {} not read: {}", skipped.path_id, skipped.error);
}
```

`sprites` returns sprites ordered by the texture that holds them, so `export` decodes each
texture (a 4096x4096 atlas is 64 MB decoded) once. To use several threads, group sprites by
`texture_id`, then per group call `decode_texture` once and `cut` for each sprite; both take
`&self`.

`Assets::open` also takes an asset bundle holding one serialized file, which is the usual case,
and textures can be listed and decoded directly:

```rust,ignore
let assets = unity_bundle_assets::Assets::open("assetbundles/characters")?;
for texture in assets.textures(|_| true) {
    let image = assets.decode_texture(texture.path_id)?; // RGBA8, top row first
}
```

For a bundle holding several serialized files (scene bundles), parse it with `Bundle::parse`
and open each with `Assets::from_bundle`.

## What it handles

- Serialized file format versions 17 to 22, little- or big-endian. Object layouts are
  hard-coded rather than read from type trees, because player builds usually strip them.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
  A file whose engine version was stripped takes its bundle's.
- `Texture2D` from Unity 5.5 through 6000.5, with pixels inline or streamed from a `.resS` file
  beside the serialized file or inside the same bundle. Fields are gated by engine release,
  checked against the engine's per-release type trees. Later releases are refused rather than
  guessed at.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 through 6000.5.
- Packing rotation and flips, and tight packing (pixels outside the sprite's mesh are cleared,
  colour and alpha).
- Texture formats Alpha8, RGB24, RGBA32, ARGB32, BGRA32, DXT1 (BC1), DXT5 (BC3).

Exports follow AssetStudio: the sprite's texture rectangle, not padded out to its full rect;
Alpha8 as white with the stored alpha (UnityPy gives black); `Rotate90` packing undone
counter-clockwise (UnityPy turns the other way, and no real sample here settles it).

## What it doesn't handle (yet)

Each of these gives an error naming it, never a wrong image:

- Encrypted bundles, the older `UnityWeb` / `UnityRaw` containers, and bundles whose textures
  stream from a different bundle.
- `Sprite` and `SpriteAtlas` before Unity 2019.1; anything newer than 6000.5.
- Every texture format not listed above, among them crunch-compressed, BC4-7, ETC, EAC, ASTC,
  PVRTC, RGB565, ARGB4444, RGBA4444, R8, R16, RG16 and the half- and float-precision formats.
- Textures swizzled for Nintendo Switch.
- Sprites whose alpha is in a separate texture, sprites in a downscaled atlas, and tight-packed
  sprites whose mesh uses a vertex layout other than float positions.
- Textures and atlases that live in another serialized file.

## Untrusted input

Files are treated as hostile. Counts and lengths are checked against the bytes present before
they size an allocation. What the data cannot bound is held to `Limits`, which a caller can
lower: file size (2 GiB), decompressed bundle size (1 GiB), texture pixels (8192 x 8192),
triangles per sprite (65,536) and per file (4M), and work spent masking one sprite. Streamed
pixels are read only from a regular file directly beside the asset file, or from the same
bundle: a stream path with a directory, a root or `..` in it is refused.

Malformed input is meant to give an error rather than a panic. It is fuzzed for that, which is
evidence, not proof.

## Tested against

- A Unity 2022.3 macOS player build: all 9,436 sprites export. Against UnityPy, 8,880 are
  pixel-identical, 555 differ only in the colour of fully transparent pixels (masking clears
  it; UnityPy keeps the texel), and one differs in 8 pixels at a mask edge, where the two
  rasterise a triangle's border differently.
- 737 asset bundles from a Unity 5.6.6, 5.6.7 and 2018.4 (.2, .11, .36) game and its mods: all
  open, and 15,193 textures export; the one failure is a dynamic font texture stored empty. On
  a sample of 171 bundles, 3,042 of the 3,056 textures UnityPy could decode are pixel-identical
  to its output, and the other 14 are the Alpha8 colour convention above, with identical alpha.
  Four of those bundles repacked as LZMA export byte-identical PNGs.
- The tests build serialized files and bundles byte by byte, since game files cannot ship with
  the crate: `Texture2D` in the layouts of eleven engine releases from 5.6 to 6000.5, sprites
  with every packing rotation, tight masks, atlases, big-endian files, every bundle container
  variant, and hostile files for each of the limits above.

The examples are the tools used to work out and check the layouts:

```sh
cargo run --example list -- <file> [class-id]           # objects by class, with names
cargo run --example dump -- <file> <path-id> [bytes]     # hex dump of one object
cargo run --example survey -- <file>                     # sprites by texture format and packing
cargo run --release --example export -- <file> <out-dir> [name-prefix...]
cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]
```

## Minimum Rust version

1.83 for the library. The tests and examples use the `image` crate, which needs 1.88.

## License

MIT. The bundle reader follows UnityPy (MIT); see NOTICE.
