# Changelog

## 0.1.0 (unreleased)

First release. Minimum Rust version 1.83.

- Serialized files, format versions 17 to 22, little- or big-endian.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
- `Texture2D` from Unity 5.5 through 6000.4: pixels inline or streamed from a `.resS` beside
  the file or inside the same bundle. Alpha8, ARGB4444, RGBA4444, RGB565, RGB24, RGBA32,
  ARGB32, BGRA32, DXT1 and DXT5.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 through 6000.4: packing rotation, flips, tight
  packing.
- `Assets` lists and exports sprites and textures, top row first; the lower layers are public.
- `Limits` bounds file size, decompression (with LZMA's working memory), objects (across a
  bundle), decoded pixels, sprite meshes, mask work and the total work done, shared by every
  `Assets` opened from one bundle.
- Hostile input: records, bundle entries, sprite sub-meshes and streamed pixel ranges may not
  overlap, whatever path spelling reaches them (on Unix by device and inode); strings are held
  to 4 KiB (versions and the paths used to find data must be UTF-8); files are checked by
  header before they are read; stream files must be regular files beside the asset, with
  plain ASCII names, no device names or data streams, and (on Unix) no links, opened without
  blocking.
