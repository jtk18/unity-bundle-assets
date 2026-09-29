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
    /// Blue, green, red, one byte each.
    pub const BGR24: i32 = 8;
    /// Red only, a little-endian 16-bit value. Decodes to red, with green and blue 0.
    pub const R16: i32 = 9;
    /// BC2: 4x4 blocks of 16 bytes, four bits of alpha a pixel.
    pub const DXT3: i32 = 11;
    /// Red only, a half-precision float. Floats decode clamped to 0-1, green and blue 0 where
    /// the format has none.
    pub const R_HALF: i32 = 15;
    /// Red and green, half-precision floats.
    pub const RG_HALF: i32 = 16;
    /// Red, green, blue, alpha, half-precision floats.
    pub const RGBA_HALF: i32 = 17;
    /// Red only, a single-precision float.
    pub const R_FLOAT: i32 = 18;
    /// Red and green, single-precision floats.
    pub const RG_FLOAT: i32 = 19;
    /// Red, green, blue, alpha, single-precision floats.
    pub const RGBA_FLOAT: i32 = 20;
    /// Red, green, blue with nine bits each and a shared five-bit exponent.
    pub const RGB9E5_FLOAT: i32 = 22;
    /// BC4: 4x4 blocks of 8 bytes, one interpolated channel. Decodes to red.
    pub const BC4: i32 = 26;
    /// BC5: 4x4 blocks of 16 bytes, two interpolated channels. Decodes to red and green.
    pub const BC5: i32 = 27;
    /// BC7: 4x4 blocks of 16 bytes in eight modes, colour with or without alpha.
    pub const BC7: i32 = 25;
    /// Red and green, one byte each.
    pub const RG16: i32 = 62;
    /// Red only, one byte.
    pub const R8: i32 = 63;
    /// Red and green, little-endian 16-bit values.
    pub const RG32: i32 = 72;
    /// Red, green, blue, little-endian 16-bit values.
    pub const RGB48: i32 = 73;
    /// Red, green, blue, alpha, little-endian 16-bit values.
    pub const RGBA64: i32 = 74;
}

/// Bytes in the first mip level, or `None` for a format this crate does not decode or a size
/// that does not fit in memory.
#[must_use]
pub fn mip0_size(format: i32, width: u32, height: u32) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(4).checked_mul(h.div_ceil(4))?;
    let pixels = w.checked_mul(h)?;
    match block_bytes(format) {
        Some(n) => blocks.checked_mul(n),
        None => pixels.checked_mul(pixel_format(format)?.0),
    }
}

/// Bytes in each 4x4 block, for a format stored in blocks.
const fn block_bytes(format: i32) -> Option<usize> {
    match format {
        format::DXT1 | format::BC4 => Some(8),
        format::DXT3 | format::DXT5 | format::BC5 | format::BC7 => Some(16),
        _ => None,
    }
}

/// Whether `format` is stored in 4x4 blocks.
pub(crate) const fn is_block_format(format: i32) -> bool {
    block_bytes(format).is_some()
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
        format::DXT3 => blocks(data, &mut out, w, h, 16, |b, px| {
            color_block(&b[8..], px, false);
            let bits = u64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8]));
            for (i, p) in px.iter_mut().enumerate() {
                p[3] = widen((bits >> (i * 4)) as u16 & 15, 15);
            }
        }),
        format::BC7 => blocks(data, &mut out, w, h, 16, crate::bc7::block),
        format::BC4 => blocks(data, &mut out, w, h, 8, |b, px| {
            let red = interpolated(b);
            for (p, r) in px.iter_mut().zip(red) {
                *p = [r, 0, 0, 255];
            }
        }),
        format::BC5 => blocks(data, &mut out, w, h, 16, |b, px| {
            let (red, green) = (interpolated(&b[..8]), interpolated(&b[8..]));
            for (i, p) in px.iter_mut().enumerate() {
                *p = [red[i], green[i], 0, 255];
            }
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
    // A format wider than RGBA8 narrows and needs no more room.
    let more = out_len.saturating_sub(size);
    data.try_reserve_exact(more)
        .map_err(|_| Error::OutOfMemory { bytes: more as u64 })?;
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
        // Any other format converts through the table `decode` uses, still in place: a wider
        // pixel narrows from the first pixel on (pixel i's output ends at or before where
        // pixel i + 1 is stored), a narrower one widens from the last back.
        _ => {
            let Some((bpp, convert)) = pixel_format(format) else {
                return decode(format, width, height, &data);
            };
            let mut px = [0u8; 4];
            let mut stored = [0u8; 16];
            let pixels = w * h;
            if bpp >= 4 {
                for i in 0..pixels {
                    stored[..bpp].copy_from_slice(&data[i * bpp..(i + 1) * bpp]);
                    convert(&stored[..bpp], &mut px);
                    data[i * 4..(i + 1) * 4].copy_from_slice(&px);
                }
                data.truncate(out_len);
            } else {
                data.resize(out_len, 0);
                for i in (0..pixels).rev() {
                    stored[..bpp].copy_from_slice(&data[i * bpp..(i + 1) * bpp]);
                    convert(&stored[..bpp], &mut px);
                    data[i * 4..(i + 1) * 4].copy_from_slice(&px);
                }
            }
        }
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
        format::BGR24 => (3, |s, d| d.copy_from_slice(&[s[2], s[1], s[0], 255])),
        format::R8 => (1, |s, d| d.copy_from_slice(&[s[0], 0, 0, 255])),
        format::RG16 => (2, |s, d| d.copy_from_slice(&[s[0], s[1], 0, 255])),
        format::R16 => (2, |s, d| d.copy_from_slice(&[unorm16(s), 0, 0, 255])),
        format::RG32 => (4, |s, d| {
            d.copy_from_slice(&[unorm16(s), unorm16(&s[2..]), 0, 255]);
        }),
        format::RGB48 => (6, |s, d| {
            d.copy_from_slice(&[unorm16(s), unorm16(&s[2..]), unorm16(&s[4..]), 255]);
        }),
        format::RGBA64 => (8, |s, d| {
            d.copy_from_slice(&[
                unorm16(s),
                unorm16(&s[2..]),
                unorm16(&s[4..]),
                unorm16(&s[6..]),
            ]);
        }),
        format::R_HALF => (2, |s, d| {
            d.copy_from_slice(&[unorm_float(half(s)), 0, 0, 255]);
        }),
        format::RG_HALF => (4, |s, d| {
            d.copy_from_slice(&[unorm_float(half(s)), unorm_float(half(&s[2..])), 0, 255]);
        }),
        format::RGBA_HALF => (8, |s, d| {
            d.copy_from_slice(&[0, 2, 4, 6].map(|i| unorm_float(half(&s[i..]))));
        }),
        format::R_FLOAT => (4, |s, d| {
            d.copy_from_slice(&[unorm_float(single(s)), 0, 0, 255]);
        }),
        format::RG_FLOAT => (8, |s, d| {
            d.copy_from_slice(&[unorm_float(single(s)), unorm_float(single(&s[4..])), 0, 255]);
        }),
        format::RGBA_FLOAT => (16, |s, d| {
            d.copy_from_slice(&[0, 4, 8, 12].map(|i| unorm_float(single(&s[i..]))));
        }),
        format::RGB9E5_FLOAT => (4, |s, d| d.copy_from_slice(&rgb9e5(s))),
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
    for (px, a) in out.iter_mut().zip(interpolated(b)) {
        px[3] = a;
    }
}

/// One interpolated channel as BC3 stores alpha (and BC4 and BC5 their channels): two
/// endpoints and sixteen 3-bit indices.
fn interpolated(b: &[u8]) -> [u8; 16] {
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
    std::array::from_fn(|i| palette[(bits >> (i * 3)) as usize & 7])
}

/// A 16-bit channel as a byte, rounded to nearest.
fn unorm16(s: &[u8]) -> u8 {
    let v = u32::from(u16::from_le_bytes([s[0], s[1]]));
    ((v * 255 + 32_895) >> 16) as u8
}

/// A float channel as a byte: clamped to 0-1, rounded to nearest (ties to even); NaN is 0.
fn unorm_float(v: f32) -> u8 {
    (v * 255.0).round_ties_even() as u8
}

fn half(s: &[u8]) -> f32 {
    let bits = u16::from_le_bytes([s[0], s[1]]);
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let fraction = u32::from(bits & 0x3ff);
    match exponent {
        0 => {
            // Zero or subnormal: fraction * 2^-24.
            let m = fraction as f32 * (1.0 / 16_777_216.0);
            if sign == 0 {
                m
            } else {
                -m
            }
        }
        0x1f => f32::from_bits(sign | 0x7f80_0000 | (fraction << 13)),
        _ => f32::from_bits(sign | ((exponent + 112) << 23) | (fraction << 13)),
    }
}

const fn single(s: &[u8]) -> f32 {
    f32::from_le_bytes([s[0], s[1], s[2], s[3]])
}

fn rgb9e5(s: &[u8]) -> [u8; 4] {
    let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    // 2^(exponent - 15 - 9), exactly.
    let scale = f32::from_bits((((v >> 27) & 31) + 127 - 24) << 23);
    let c = |shift: u32| unorm_float(((v >> shift) & 0x1ff) as f32 * scale);
    [c(0), c(9), c(18), 255]
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
            format::BGR24,
            format::R8,
            format::RG16,
            format::R16,
            format::RG32,
            format::RGB48,
            format::RGBA64,
            format::R_HALF,
            format::RG_HALF,
            format::RGBA_HALF,
            format::R_FLOAT,
            format::RG_FLOAT,
            format::RGBA_FLOAT,
            format::RGB9E5_FLOAT,
        ] {
            let size = mip0_size(format, 5, 3).unwrap();
            for extra in [0, 12] {
                // Wider formats narrow into the pixels' own room.
                let mut data = Vec::with_capacity((5 * 3 * 4).max(size) + extra);
                data.extend((0..size + extra).map(|i| (i * 37 % 251) as u8));
                let at = data.as_ptr();
                let want = decode(format, 5, 3, &data).unwrap();
                let got = decode_owned(format, 5, 3, data).unwrap();
                assert_eq!(got.as_ptr(), at, "format {format}, {extra} extra");
                assert_eq!(got, want, "format {format}, {extra} extra");
            }
        }
    }

    #[test]
    fn test_wide_and_float_pixels() {
        let one = |format, stored: &[u8]| decode(format, 1, 1, stored).unwrap();
        assert_eq!(one(format::BGR24, &[1, 2, 3]), [3, 2, 1, 255]);
        assert_eq!(one(format::R8, &[9]), [9, 0, 0, 255]);
        assert_eq!(one(format::RG16, &[9, 8]), [9, 8, 0, 255]);
        // 16-bit channels round to nearest: 0x8080 is 128.5 levels of 255, 0x7f7f 127.5.
        assert_eq!(one(format::R16, &[0xff, 0xff]), [255, 0, 0, 255]);
        assert_eq!(
            one(format::RGB48, &[0x80, 0x80, 0x7f, 0x7f, 0, 0]),
            [128, 127, 0, 255]
        );
        // Floats clamp to 0-1 and round half to even; NaN is 0.
        let f = |v: f32| v.to_le_bytes();
        let px = [f(0.5), f(-1.0), f(7.0), f(f32::NAN)].concat();
        assert_eq!(one(format::RGBA_FLOAT, &px), [128, 0, 255, 0]);
        // Halves: 1.0 is 0x3c00, 0.5 is 0x3800, infinity 0x7c00.
        let px = [0x00, 0x3c, 0x00, 0x38, 0x00, 0x7c, 0x00, 0x00];
        assert_eq!(one(format::RGBA_HALF, &px), [255, 128, 255, 0]);
        // RGB9E5: mantissas 256, 128, 0 with exponent 16 give 2^-8 each step: 1.0, 0.5, 0.
        let v: u32 = 256 | (128 << 9) | (16 << 27);
        assert_eq!(
            one(format::RGB9E5_FLOAT, &v.to_le_bytes()),
            [255, 128, 0, 255]
        );
    }

    #[test]
    fn test_dxt3_bc4_bc5_blocks() {
        let red = 0xf800u16.to_le_bytes();
        let colour = [red[0], red[1], red[0], red[1], 0, 0, 0, 0];
        // DXT3: pixel 0's alpha nibble 3, pixel 1's 15, the rest 0.
        let block = [[0xf3, 0, 0, 0, 0, 0, 0, 0], colour].concat();
        let out = decode(format::DXT3, 4, 4, &block).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [255, 0, 0, 51]);
        assert_eq!(at(&out, 4, 4, 1, 0), [255, 0, 0, 255]);
        assert_eq!(at(&out, 4, 4, 2, 0), [255, 0, 0, 0]);
        // BC4: endpoints 200 and 100, pixel 0 index 1 (the second endpoint), pixel 1 index 2.
        let channel = [200, 100, 0b010_001, 0, 0, 0, 0, 0];
        let out = decode(format::BC4, 4, 4, &channel).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [100, 0, 0, 255]);
        assert_eq!(at(&out, 4, 4, 1, 0), [185, 0, 0, 255]);
        // BC5: red then green, each a BC4 block.
        let other = [50, 60, 0, 0, 0, 0, 0, 0];
        let out = decode(format::BC5, 4, 4, &[channel, other].concat()).unwrap();
        assert_eq!(at(&out, 4, 4, 0, 0), [100, 50, 0, 255]);
    }

    #[test]
    fn test_bc7_every_mode() {
        // 64 pseudo-random blocks, block i in mode i % 9 (the ninth the reserved mode, first
        // byte 0). The hash was taken when this output matched unity-rs-core 0.5.2's decoder
        // (texture2ddecoder) byte for byte.
        let mut s = 0x2545_f491_4f6c_dd1d_u64;
        let mut data: Vec<u8> = (0..64 * 16)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect();
        for (i, b) in data.chunks_exact_mut(16).enumerate() {
            let m = i % 9;
            b[0] = if m == 8 {
                0
            } else {
                b[0].checked_shl(m as u32 + 1).unwrap_or(0) | (1 << m)
            };
        }
        let out = decode(format::BC7, 32, 32, &data).unwrap();
        let hash = out.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &x| {
            (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3)
        });
        assert_eq!(hash, 0x0a1e_fe0c_f8b9_c5e7);
        // A reserved-mode block is transparent black.
        let mut px = [[1u8; 4]; 16];
        crate::bc7::block(&[0; 16], &mut px);
        assert_eq!(px, [[0; 4]; 16]);
        // Mode 6, every endpoint bit and p-bit set: opaque white whatever the indices.
        let mut block = [0xffu8; 16];
        block[0] = 0b1100_0000;
        crate::bc7::block(&block, &mut px);
        assert_eq!(px, [[255; 4]; 16]);
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
