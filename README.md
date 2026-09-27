# unity-sprites

Read Unity serialized asset files (`*.assets`, `level*`) and asset bundles, and export their
sprites and textures as RGBA images, in pure Rust. No .NET, no native libraries, no external
tools.

```rust
let mut assets = unity_sprites::Assets::open("Game_Data/sharedassets0.assets".as_ref())?;
for sprite in assets.sprites(|name| name.starts_with("Icon_"))? {
    let image = assets.export(&sprite)?; // RGBA8, top row first
    // hand image.rgba to the `image` crate, a GPU upload, ...
}
```

`sprites` returns sprites ordered by the texture that holds them, so `export` decodes each
texture (a 4096x4096 atlas is 64 MB decoded) once.

To use several threads, group sprites by `texture_id`, then per group call `decode_texture` once
and `cut` for each sprite. Both take `&self`.

`Assets::open` also takes an asset bundle holding one serialized file, which is the usual case.
Textures are listed and decoded directly:

```rust
let assets = unity_sprites::Assets::open("assetbundles/characters".as_ref())?;
for (path_id, name) in assets.textures(|_| true) {
    let image = assets.decode_texture(path_id)?; // RGBA8, bottom row first, as Unity stores it
}
```

For a bundle holding several serialized files (scene bundles), parse it with `Bundle::parse`
and open each with `Assets::from_bundle`.

## What it handles

- Serialized file format versions 17 to 22, little- or big-endian, with or without type
  trees. Object layouts are hard-coded rather than read from type trees, because player
  builds usually strip them.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
- `Texture2D` from Unity 5.5 on, with pixels inline or streamed from a `.resS` file beside the
  serialized file or inside the same bundle. Fields are gated by the exact engine release they
  appeared in.
- `Sprite` from Unity 2019.1 on, including sprites packed into a `SpriteAtlas`.
- Packing rotation and flips, and tight packing (pixels outside the sprite's mesh are cleared).
- Texture formats Alpha8, RGB24, RGBA32, ARGB32, BGRA32, DXT1 (BC1), DXT5 (BC3).

Exports match what AssetStudio produces: the sprite's texture rectangle, not padded out to
its full rect. Alpha8 textures decode to white with the stored alpha (UnityPy gives black).

## What it doesn't handle (yet)

- Encrypted bundles, the older `UnityWeb` / `UnityRaw` containers, and bundles whose textures
  stream from a different bundle.
- `Sprite` before Unity 2019.1.
- Crunch-compressed, BC4-7, ETC, ASTC and PVRTC textures. These return
  `Error::Unsupported`, naming the format.
- Textures that live in another serialized file.
- Atlas `downscaleMultiplier` values other than 1.

## Tested against

Mechabellum (Unity 2022.3.62f3, macOS build): all 9,436 sprites in `sharedassets0.assets`
read. The 1,178 exported with name-prefix filters decoded and checked by eye.

737 asset bundles from a Unity 5.6.6, 5.6.7 and 2018.4 (.2, .11, .36) game and its mods: all
open, and 15,193 textures export; the one failure is a dynamic font texture stored empty. On a
sample of 171 bundles, 3,042 of 3,057 textures (DXT1, DXT5, RGBA32, RGB24, Alpha8) are
pixel-identical to UnityPy's output; the other 14 are the Alpha8 colour convention above, with
identical alpha. Four of those bundles repacked as LZMA export byte-identical PNGs.

The examples (`list`, `dump`, `survey`, `export`, `textures`) are the tools used to work out and
check the layouts:

```sh
cargo run --example list -- <file> [class-id]           # objects by class, with names
cargo run --example dump -- <file> <path-id> [bytes]     # hex dump of one object
cargo run --example survey -- <file>                     # sprites by texture format and packing
cargo run --release --example export -- <file> <out-dir> [name-prefix...]
cargo run --release --example textures -- <file-or-bundle> <out-dir> [name-prefix...]
```

## License

MIT. The bundle reader follows UnityPy (MIT); see NOTICE.
