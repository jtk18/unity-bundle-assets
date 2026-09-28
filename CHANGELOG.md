# Changelog

## 0.1.0 (unreleased)

First release, as `unity-bundle-assets`. Development ran as `unity-sprites`.

- Serialized files, format versions 17 to 22, little- or big-endian.
- Asset bundles (`UnityFS`), with blocks stored plain or compressed with LZ4, LZ4HC or LZMA.
- `Texture2D` from Unity 5.5 on, pixels inline or streamed from a `.resS` beside the file or
  inside the same bundle. Alpha8, RGB24, RGBA32, ARGB32, BGRA32, DXT1 and DXT5.
- `Sprite` and `SpriteAtlas` from Unity 2019.1 on: packing rotation, flips, tight packing.
- `Assets` lists and exports sprites and textures; the lower layers are public.

A note on history: commit `881fb6a` said it corrected the `Texture2D` gates for
`m_IgnoreMasterTextureLimit` and `m_IsPreProcessed`. The new gates match the engine's type
trees, but the old ones never misread a texture: both fields are single bytes in a run the
reader aligns to four bytes straight after, so reading one more or one fewer lands in the
same place.
