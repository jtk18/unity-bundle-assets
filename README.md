# unity-sprites

Read Unity serialized asset files (`*.assets`, `level*`) and export their sprites as RGBA
images, in pure Rust. No .NET, no native libraries, no external tools.

```rust
let mut assets = unity_sprites::Assets::open("Game_Data/sharedassets0.assets".as_ref())?;
for sprite in assets.sprites(|name| name.starts_with("Icon_"))? {
    let image = assets.export(&sprite)?; // RGBA8, top row first
    // hand image.rgba to the `image` crate, a GPU upload, ...
}
```

`sprites` returns sprites ordered by the texture that holds them, so `export` decodes each
texture (a 4096x4096 atlas is 64 MB decoded) once.

## What it handles

- Serialized file format versions 17 to 22, little- or big-endian, with or without type
  trees. Object layouts are hard-coded rather than read from type trees, because player
  builds usually strip them.
- `Texture2D` from Unity 2019.3 on, with pixels inline or streamed from a `.resS` file.
- `Sprite` from Unity 2019.1 on, including sprites packed into a `SpriteAtlas`.
- Packing rotation and flips, and tight packing (pixels outside the sprite's mesh are cleared).
- Texture formats Alpha8, RGB24, RGBA32, ARGB32, BGRA32, DXT1 (BC1), DXT5 (BC3).

Exports match what AssetStudio produces: the sprite's texture rectangle, not padded out to
its full rect.

## What it doesn't handle (yet)

- Asset bundles (`UnityFS`) and textures streamed from `archive:` paths.
- Crunch-compressed, BC4-7, ETC, ASTC and PVRTC textures. These return
  `Error::Unsupported`, naming the format.
- Textures that live in another serialized file.
- Atlas `downscaleMultiplier` values other than 1.

## Tested against

Mechabellum (Unity 2022.3.62f3, macOS build): all 9,436 sprites in `sharedassets0.assets`
read. The 1,178 exported with name-prefix filters decoded and checked by eye.

The examples (`list`, `dump`, `survey`, `export`) are the tools used to work out and check
the layouts:

```sh
cargo run --example list -- <file> [class-id]           # objects by class, with names
cargo run --example dump -- <file> <path-id> [bytes]     # hex dump of one object
cargo run --example survey -- <file>                     # sprites by texture format and packing
cargo run --release --example export -- <file> <out-dir> [name-prefix...]
```

## License

MIT
