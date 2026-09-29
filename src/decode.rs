//! Pixel formats to RGBA8. Unity stores rows bottom first; decoding turns them the right way
//! up, so output rows are top first, like every image this crate returns.

use crate::{Error, Result};

/// The `TextureFormat` values this crate decodes.
pub mod format {
    /// Alpha only, one byte. Decodes to white with that alpha, as `AssetStudio` does (`UnityPy`
    /// gives black).
    pub const ALPHA8: i32 = 1;
    /// Alpha, red, green, blue, four bits each in a little-endian 16-bit word (alpha in the
    /// top bits).
    pub const ARGB4444: i32 = 2;
    /// Red, green, blue, one byte each.
    pub const RGB24: i32 = 3;
    /// Red, green, blue, alpha, one byte each.
    pub const RGBA32: i32 = 4;
    /// Alpha, red, green, blue, one byte each.
    pub const ARGB32: i32 = 5;
    /// Red 5 bits, green 6, blue 5 in a little-endian 16-bit word (red in the top bits).
    /// Widened as Pillow (and so `UnityPy`) does, rounding down, which can be a level darker
    /// than a GPU shows.
    pub const RGB565: i32 = 7;
    /// BC1: 4x4 blocks of 8 bytes, one bit of alpha.
    pub const DXT1: i32 = 10;
    /// BC3: 4x4 blocks of 16 bytes, interpolated alpha.
    pub const DXT5: i32 = 12;
    /// Red, green, blue, alpha, four bits each in a little-endian 16-bit word (red in the top
    /// bits).
    pub const RGBA4444: i32 = 13;
    /// Blue, green, red, alpha, one byte each.
    pub const BGRA32: i32 = 14;
}

/// Bytes in the first mip level, or `None` for a format this crate does not decode or a size
/// that does not fit in memory.
#[must_use]
pub fn mip0_size(format: i32, width: u32, height: u32) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(4).checked_mul(h.div_ceil(4))?;
    let pixels = w.checked_mul(h)?;
    match format {
        format::ALPHA8 => Some(pixels),
        format::ARGB4444 | format::RGB565 | format::RGBA4444 => pixels.checked_mul(2),
        format::RGB24 => pixels.checked_mul(3),
        format::RGBA32 | format::ARGB32 | format::BGRA32 => pixels.checked_mul(4),
        format::DXT1 => blocks.checked_mul(8),
        format::DXT5 => blocks.checked_mul(16),
        _ => None,
    }
}

/// Whether `format` is stored in 4x4 blocks (DXT1, DXT5).
pub(crate) const fn is_block_format(format: i32) -> bool {
    matches!(format, format::DXT1 | format::DXT5)
}

/// Whether this crate decodes `format`.
#[must_use]
pub fn is_supported(format: i32) -> bool {
    mip0_size(format, 4, 4).is_some()
}

/// Decode the first mip level, stored bottom row first as Unity does, to RGBA8 top row first.
///
/// Extra bytes (further mip levels) are ignored. Sizes are not limited here beyond what fits
/// in memory; [`crate::Assets::decode_texture`] applies [`crate::Limits`].
///
/// # Errors
///
/// [`Error::UnsupportedTextureFormat`] for a format this crate does not decode;
/// [`Error::InvalidArgument`] for a size that cannot fit in memory or data shorter than the
/// first mip level (the arguments' fault, even when they came from a file);
/// [`Error::OutOfMemory`] when the output cannot be allocated.
pub fn decode(format: i32, width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    if !is_supported(format) {
        return Err(Error::UnsupportedTextureFormat { name: None, format });
    }
    let (w, h) = (width as usize, height as usize);
    let out_len = w
        .checked_mul(h)
        .and_then(|p| p.checked_mul(4))
        .filter(|&n| isize::try_from(n).is_ok());
    let (Some(size), Some(out_len)) = (mip0_size(format, width, height), out_len) else {
        return Err(Error::InvalidArgument(format!(
            "a {width}x{height} texture is too large to decode"
        )));
    };
    let data = data.get(..size).ok_or_else(|| {
        Error::InvalidArgument(format!("{} bytes of pixels, need {size}", data.len()))
    })?;
    if out_len == 0 {
        return Ok(Vec::new());
    }
    let mut out = crate::zeroed(out_len)?;
    match format {
        format::DXT1 => blocks(data, &mut out, w, h, 8, |b, px| color_block(b, px, true)),
        format::DXT5 => blocks(data, &mut out, w, h, 16, |b, px| {
            color_block(&b[8..], px, false);
            alpha_block(&b[..8], px);
        }),
        _ => {
            let Some((bpp, convert)) = pixel_format(format) else {
                return Err(Error::UnsupportedTextureFormat { name: None, format });
            };
            // Stored row y becomes output row h - 1 - y.
            for (y, src) in data.chunks_exact((w * bpp).max(1)).take(h).enumerate() {
                let row = &mut out[(h - 1 - y) * w * 4..][..w * 4];
                if format == format::RGBA32 {
                    row.copy_from_slice(src);
                } else {
                    for (s, d) in src.chunks_exact(bpp).zip(row.chunks_exact_mut(4)) {
                        convert(s, d);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// A little-endian 16-bit word's four nibbles, top first, each widened to a byte.
fn nibbles(s: &[u8]) -> [u8; 4] {
    let v = u16::from_le_bytes([s[0], s[1]]);
    [v >> 12, v >> 8, v >> 4, v].map(|n| widen(n & 15, 15))
}

/// A channel of `max + 1` levels as a byte, rounding down as Pillow (and so `UnityPy`) does.
const fn widen(value: u16, max: u16) -> u8 {
    (value as u32 * 255 / max as u32) as u8
}

/// [`decode`], taking ownership of the pixels: data stored pixel by pixel (not in blocks)
/// and holding at least the first mip is converted to RGBA8 in its own buffer (four-byte
/// formats reordered where they lie, narrower ones widened from the last pixel back) and
/// turned the right way up, so decoding it needs no second buffer. The buffer should have
/// room for the result already, or growing it may copy it.
pub(crate) fn decode_owned(
    format: i32,
    width: u32,
    height: u32,
    mut data: Vec<u8>,
) -> Result<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let out_len = w
        .checked_mul(h)
        .and_then(|p| p.checked_mul(4))
        .filter(|&n| isize::try_from(n).is_ok());
    let size = mip0_size(format, width, height);
    let (true, Some(size), Some(out_len)) = (pixel_format(format).is_some(), size, out_len) else {
        return decode(format, width, height, &data);
    };
    if data.len() < size {
        return decode(format, width, height, &data);
    }
    data.truncate(size);
    data.try_reserve_exact(out_len - size)
        .map_err(|_| Error::OutOfMemory {
            bytes: (out_len - size) as u64,
        })?;
    match format {
        // Already four bytes a pixel: reordered where they lie, or left as they are.
        format::RGBA32 => {}
        format::BGRA32 => data.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)),
        // A, R, G, B read little-endian is one word; turning it a byte right gives R, G, B, A.
        format::ARGB32 => data.chunks_exact_mut(4).for_each(|p| {
            let word = u32::from_le_bytes([p[0], p[1], p[2], p[3]]).rotate_right(8);
            p.copy_from_slice(&word.to_le_bytes());
        }),
        format::ALPHA8 => widen_pixels::<1>(&mut data, out_len, |s| [255, 255, 255, s[0]]),
        format::RGB24 => widen_pixels::<3>(&mut data, out_len, |s| [s[0], s[1], s[2], 255]),
        format::ARGB4444 => widen_pixels::<2>(&mut data, out_len, |s| {
            let argb = nibbles(&s);
            [argb[1], argb[2], argb[3], argb[0]]
        }),
        format::RGBA4444 => widen_pixels::<2>(&mut data, out_len, |s| nibbles(&s)),
        format::RGB565 => widen_pixels::<2>(&mut data, out_len, |s| {
            let v = u16::from_le_bytes(s);
            [
                widen(v >> 11, 31),
                widen((v >> 5) & 63, 63),
                widen(v & 31, 31),
                255,
            ]
        }),
        // Any other format is decoded by copying, from the table `decode` uses.
        _ => return decode(format, width, height, &data),
    }
    let row = w * 4;
    for i in 0..h / 2 {
        let (top, bottom) = data.split_at_mut((h - 1 - i) * row);
        top[i * row..(i + 1) * row].swap_with_slice(&mut bottom[..row]);
    }
    Ok(data)
}

/// Widen `data`, `N` stored bytes a pixel, to `len` bytes of RGBA8 in place, from the last
/// pixel back: pixel i's four output bytes start at 4i, at or after where its stored bytes and
/// every later pixel's start, so nothing is overwritten unread.
fn widen_pixels<const N: usize>(
    data: &mut Vec<u8>,
    len: usize,
    convert: impl Fn([u8; N]) -> [u8; 4],
) {
    let pixels = data.len() / N;
    data.resize(len, 0);
    for i in (0..pixels).rev() {
        let mut stored = [0u8; N];
        stored.copy_from_slice(&data[i * N..(i + 1) * N]);
        data[i * 4..(i + 1) * 4].copy_from_slice(&convert(stored));
    }
}

/// The stored bytes a pixel and the conversion to RGBA8 for a format stored pixel by pixel
/// (not in blocks).
fn pixel_format(format: i32) -> Option<(usize, Convert)> {
    Some(match format {
        format::ALPHA8 => (1, |s, d| d.copy_from_slice(&[255, 255, 255, s[0]])),
        format::RGB24 => (3, |s, d| d.copy_from_slice(&[s[0], s[1], s[2], 255])),
        format::RGBA32 => (4, |s, d| d.copy_from_slice(s)),
        format::ARGB32 => (4, |s, d| d.copy_from_slice(&[s[1], s[2], s[3], s[0]])),
        format::ARGB4444 => (2, |s, d| {
            let argb = nibbles(s);
            d.copy_from_slice(&[argb[1], argb[2], argb[3], argb[0]]);
        }),
        format::RGBA4444 => (2, |s, d| d.copy_from_slice(&nibbles(s))),
        format::RGB565 => (2, |s, d| {
            let v = u16::from_le_bytes([s[0], s[1]]);
            d.copy_from_slice(&[
                widen(v >> 11, 31),
                widen((v >> 5) & 63, 63),
                widen(v & 31, 31),
                255,
            ]);
        }),
        format::BGRA32 => (4, |s, d| d.copy_from_slice(&[s[2], s[1], s[0], s[3]])),
        _ => return None,
    })
}

/// Turns one stored pixel into RGBA8.
type Convert = fn(&[u8], &mut [u8]);

/// Run `decode_block` over every 4x4 block and place its pixels, turning rows the right way
/// up.
fn blocks(
    data: &[u8],
    out: &mut [u8],
    w: usize,
    h: usize,
    block_size: usize,
    decode_block: impl Fn(&[u8], &mut [[u8; 4]; 16]),
) {
    let bw = w.div_ceil(4);
    for (i, block) in data.chunks_exact(block_size).enumerate() {
        let (bx, by) = (i % bw * 4, i / bw * 4);
        let mut px = [[0u8; 4]; 16];
        decode_block(block, &mut px);
        for (j, p) in px.iter().enumerate() {
            let (x, y) = (bx + j % 4, by + j / 4);
            if x < w && y < h {
                out[((h - 1 - y) * w + x) * 4..][..4].copy_from_slice(p);
            }
        }
    }
}

const fn rgb565(c: u16) -> [u8; 3] {
    let r = (c >> 11) & 0x1f;
    let g = (c >> 5) & 0x3f;
    let b = c & 0x1f;
    [
        ((r << 3) | (r >> 2)) as u8,
        ((g << 2) | (g >> 4)) as u8,
        ((b << 3) | (b >> 2)) as u8,
    ]
}

/// BC1 colour block. In DXT1 mode, `c0 <= c1` selects the three-colour-plus-transparent
/// palette; BC3's colour block always uses four colours.
fn color_block(b: &[u8], out: &mut [[u8; 4]; 16], dxt1: bool) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u16, wb: u16| {
        ((u16::from(a) * wa + u16::from(b) * wb) / (wa + wb)) as u8
    };
    let mut palette = [[0u8; 4]; 4];
    palette[0] = [p0[0], p0[1], p0[2], 255];
    palette[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || !dxt1 {
        palette[2] = [
            mix(p0[0], p1[0], 2, 1),
            mix(p0[1], p1[1], 2, 1),
            mix(p0[2], p1[2], 2, 1),
            255,
        ];
        palette[3] = [
            mix(p0[0], p1[0], 1, 2),
            mix(p0[1], p1[1], 1, 2),
            mix(p0[2], p1[2], 1, 2),
            255,
        ];
    } else {
        palette[2] = [
            mix(p0[0], p1[0], 1, 1),
            mix(p0[1], p1[1], 1, 1),
            mix(p0[2], p1[2], 1, 1),
            255,
        ];
        palette[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = palette[(bits >> (i * 2)) as usize & 3];
    }
}

/// BC3 alpha block: two endpoints and sixteen 3-bit indices.
fn alpha_block(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (u16::from(b[0]), u16::from(b[1]));
    let mut palette = [0u8; 8];
    palette[0] = b[0];
    palette[1] = b[1];
    if a0 > a1 {
        for i in 1..7u16 {
            palette[i as usize + 1] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u16 {
            palette[i as usize + 1] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        palette[6] = 0;
        palette[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &byte) in b[2..8].iter().enumerate() {
        bits |= u64::from(byte) << (8 * i);
    }
    for (i, px) in out.iter_mut().enumerate() {
        px[3] = palette[(bits >> (i * 3)) as usize & 7];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pixel_formats_are_decoded_in_their_own_buffer() {
        // Exactly the first mip, and with a mip tail after it, in a buffer with room for the
        // result: the result is that buffer, not a copy (the memory figures rest on it).
        for format in [
            format::ALPHA8,
            format::ARGB4444,
            format::RGB24,
            format::RGBA32,
            format::ARGB32,
            format::RGB565,
            format::RGBA4444,
            format::BGRA32,
        ] {
            let size = mip0_size(format, 5, 3).unwrap();
            for extra in [0, 12] {
                let mut data = Vec::with_capacity(5 * 3 * 4 + extra);
                data.extend((0..size + extra).map(|i| (i * 37 % 251) as u8));
                let at = data.as_ptr();
                let want = decode(format, 5, 3, &data).unwrap();
                let got = decode_owned(format, 5, 3, data).unwrap();
                assert_eq!(got.as_ptr(), at, "format {format}, {extra} extra");
                assert_eq!(got, want, "format {format}, {extra} extra");
            }
        }
    }

    /// The output pixel for stored pixel (x, y), y counted from the bottom as stored.
    fn at(out: &[u8], w: usize, h: usize, x: usize, y: usize) -> [u8; 4] {
        out[((h - 1 - y) * w + x) * 4..][..4].try_into().unwrap()
    }

    #[test]
    fn test_dxt1_block_endpoints_and_transparency() {
        // c0 = pure red, c1 = pure blue, c0 > c1: four opaque colours.
        let red = 0xf800u16.to_le_bytes();
        let blue = 0x001fu16.to_le_bytes();
        // Indices: pixel 0 -> c0, pixel 1 -> c1, pixel 2 -> 2/3 c0 + 1/3 c1, rest 0.
        let block = [red[0], red[1], blue[0], blue[1], 0b10_01_00, 0, 0, 0];
        let out = decode(format::DXT1, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&out, 4, 4, 1, 0), [0, 0, 255, 255]);
        assert_eq!(at(&out, 4, 4, 2, 0), [170, 0, 85, 255]);

        // c0 < c1 in DXT1: index 3 is transparent black, index 2 the midpoint.
        let block = [blue[0], blue[1], red[0], red[1], 0b10_11, 0, 0, 0];
        let out = decode(format::DXT1, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(at(&out, 4, 4, 1, 0), [127, 0, 127, 255]);
        // c0 == c1 is the three-colour mode too.
        let block = [red[0], red[1], red[0], red[1], 0b11, 0, 0, 0];
        let out = decode(format::DXT1, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [0, 0, 0, 0]);
        // In DXT5 the colour block always has four colours.
        let mut block5 = [255u8; 16];
        block5[8..].copy_from_slice(&[red[0], red[1], red[0], red[1], 0b11, 0, 0, 0]);
        let out = decode(format::DXT5, 4, 4, &block5).unwrap();
        assert_eq!(&at(&out, 4, 4, 0, 0)[..3], &[255, 0, 0]);
    }

    #[test]
    fn test_dxt5_alpha_palettes() {
        // a0 = 255 > a1 = 0: eight-value ramp; pixel 0 -> a0, pixel 1 -> a1, pixel 2 -> 2.
        let mut block = [0u8; 16];
        block[0] = 255;
        let bits: u64 = 1 << 3 | 2 << 6;
        block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        let out = decode(format::DXT5, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0)[3], 255);
        assert_eq!(at(&out, 4, 4, 1, 0)[3], 0);
        assert_eq!(at(&out, 4, 4, 2, 0)[3], (6 * 255 / 7) as u8);
        // a0 = 0 <= a1 = 255: six-value ramp, then index 6 is 0 and index 7 is 255.
        let mut block = [0u8; 16];
        block[1] = 255;
        let bits: u64 = 2 | 6 << 3 | 7 << 6;
        block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        let out = decode(format::DXT5, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0)[3], 255 / 5);
        assert_eq!(at(&out, 4, 4, 1, 0)[3], 0);
        assert_eq!(at(&out, 4, 4, 2, 0)[3], 255);
    }

    #[test]
    fn test_byte_formats_and_row_order() {
        // 1x2 images: stored row 0 (bottom) then row 1 (top); output top first.
        let rgba = decode(format::RGBA32, 1, 2, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(rgba, [5, 6, 7, 8, 1, 2, 3, 4]);
        let argb = decode(format::ARGB32, 1, 2, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(argb, [6, 7, 8, 5, 2, 3, 4, 1]);
        let bgra = decode(format::BGRA32, 1, 2, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(bgra, [7, 6, 5, 8, 3, 2, 1, 4]);
        let rgb = decode(format::RGB24, 1, 2, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(rgb, [4, 5, 6, 255, 1, 2, 3, 255]);
        let a8 = decode(format::ALPHA8, 1, 2, &[9, 10]).unwrap();
        assert_eq!(a8, [255, 255, 255, 10, 255, 255, 255, 9]);
    }

    #[test]
    fn test_decode_owned_matches_decode() {
        let data: Vec<u8> = (0..3 * 5 * 4).map(|i| i as u8).collect();
        for (w, h) in [(3, 5), (3, 4), (1, 1)] {
            let n = w as usize * h as usize * 4;
            let mut with_mip = data[..n].to_vec();
            with_mip.extend([7; 4]);
            assert_eq!(
                decode_owned(format::RGBA32, w, h, with_mip).unwrap(),
                decode(format::RGBA32, w, h, &data[..n]).unwrap()
            );
        }
        assert!(decode_owned(format::RGBA32, 3, 5, vec![0; 10]).is_err());
        assert_eq!(
            decode_owned(format::ALPHA8, 1, 2, vec![9, 10]).unwrap(),
            decode(format::ALPHA8, 1, 2, &[9, 10]).unwrap()
        );
    }

    #[test]
    fn test_partial_blocks_and_short_data() {
        // A 5x3 DXT1 texture needs 2x1 blocks.
        assert_eq!(mip0_size(format::DXT1, 5, 3), Some(16));
        assert_eq!(
            decode(format::DXT1, 5, 3, &[0; 16]).unwrap().len(),
            5 * 3 * 4
        );
        assert!(decode(format::DXT1, 5, 3, &[0; 15]).is_err());
        assert!(matches!(
            decode(9999, 4, 4, &[0; 64]),
            Err(Error::UnsupportedTextureFormat { format: 9999, .. })
        ));
    }

    #[test]
    fn test_huge_sizes_are_errors_not_panics() {
        assert_eq!(mip0_size(format::RGBA32, u32::MAX, u32::MAX), None);
        for f in [
            format::ALPHA8,
            format::RGB24,
            format::RGBA32,
            format::DXT1,
            format::DXT5,
        ] {
            assert!(decode(f, u32::MAX, u32::MAX, &[]).is_err());
            assert!(decode(f, 1 << 31, 1 << 31, &[]).is_err());
        }
    }
}
