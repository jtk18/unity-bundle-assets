# Changelog

## 0.1.1 (2026-09-29)

- `Sprite` and `SpriteAtlas` from Unity 2018.1 (was 2019.1): no secondary textures before
  2019.1, the 2017-2018 vertex format numbering, and 2018.1's `m_SourceSkin`. The 19 real
  2018.4 sprites in the test corpus match unity-rs-core 0.5.2 up to transparent colour.
- Texture formats R8, RG16, R16, RG32, BGR24, RGB48, RGBA64, RHalf, RGHalf, RGBAHalf, RFloat,
  RGFloat, RGBAFloat, RGB9e5, DXT3, BC4, BC5 and BC7, each checked byte for byte against
  unity-rs-core 0.5.2 on random data, and BC7 on real textures too. Formats stored wider than
  RGBA8 are narrowed in their own buffer.
- LZMA blocks are decoded by the crate itself, straight into the bundle's buffer, instead of
  through lzma-rs: bundle parsing is about 1.8 times as fast (100 real bundles, 2.4 GB out:
  4.0 s to 2.2 s), and the only runtime dependency left is `thiserror`. Output is identical
  to 0.1.0's over 660 real bundles (40.8 GB) and 24,000 corrupted ones.
- LZ4 blocks are decoded into a buffer sized up front, with fixed 16-byte copies for short
  runs.

## 0.1.0 (2026-09-29)

First release. Minimum Rust version 1.83.

- Serialized files, format versions 17 to 22, little- or big-endian.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
- `Texture2D` from Unity 5.5 through 6000.4: pixels inline or streamed from a `.resS` beside
  the file or inside the same bundle. Alpha8, ARGB4444, RGBA4444, RGB565, RGB24, RGBA32,
  ARGB32, BGRA32, DXT1 and DXT5.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 through 6000.4: packing rotation, flips, tight
  packing.
- `Assets` lists and exports sprites and textures, top row first; the lower layers are public.
- `Limits` bounds file size, decompression (with LZMA's working memory), objects and
  dependencies (across a bundle), decoded pixels, sprite meshes, mask work and the total work
  done, shared by every `Assets` opened from one bundle.
- Hostile input: records, bundle entries, sprite sub-meshes and streamed pixel ranges may not
  overlap, whatever path spelling reaches them (on Unix by device and inode); strings are held
  to 4 KiB (versions and the paths used to find data must be UTF-8); files are checked by
  header before they are read; stream files must be regular files beside the asset, with
  plain ASCII names, no device names or data streams, no symbolic links, and (on Unix) no
  other hard links, opened without blocking.
