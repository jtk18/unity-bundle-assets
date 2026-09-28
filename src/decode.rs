//! Pixel formats to RGBA8. Rows come out in the order Unity stores them: bottom row first.

use crate::{Error, Result};

/// The `TextureFormat` values this crate decodes.
pub mod format {
    /// Alpha only, one byte. Decodes to white with that alpha.
    pub const ALPHA8: i32 = 1;
    /// Red, green, blue, one byte each.
    pub const RGB24: i32 = 3;
    /// Red, green, blue, alpha, one byte each.
    pub const RGBA32: i32 = 4;
    /// Alpha, red, green, blue, one byte each.
    pub const ARGB32: i32 = 5;
    /// Blue, green, red, alpha, one byte each.
    pub const BGRA32: i32 = 14;
    /// BC1: 4x4 blocks of 8 bytes, one bit of alpha.
    pub const DXT1: i32 = 10;
    /// BC3: 4x4 blocks of 16 bytes, interpolated alpha.
    pub const DXT5: i32 = 12;
}

/// Bytes in the first mip level, or `None` for a format this crate does not decode.
pub fn mip0_size(format: i32, width: u32, height: u32) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    let blocks = w.div_ceil(4) * h.div_ceil(4);
    Some(match format {
        format::ALPHA8 => w * h,
        format::RGB24 => w * h * 3,
        format::RGBA32 | format::ARGB32 | format::BGRA32 => w * h * 4,
        format::DXT1 => blocks * 8,
        format::DXT5 => blocks * 16,
        _ => return None,
    })
}

/// Decode the first mip level to RGBA8, rows in the order stored (bottom first, for Unity).
/// Extra bytes (further mip levels) are ignored.
pub fn decode(format: i32, width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    let size = mip0_size(format, width, height)
        .ok_or_else(|| Error::Unsupported(format!("texture format {format}")))?;
    let data = data
        .get(..size)
        .ok_or_else(|| Error::Invalid(format!("{} bytes of pixels, need {size}", data.len())))?;
    let (w, h) = (width as usize, height as usize);
    Ok(match format {
        format::ALPHA8 => data.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
        format::RGB24 => data
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        format::RGBA32 => data.to_vec(),
        format::ARGB32 => data
            .chunks_exact(4)
            .flat_map(|p| [p[1], p[2], p[3], p[0]])
            .collect(),
        format::BGRA32 => data
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect(),
        format::DXT1 => blocks(data, w, h, 8, |b, out| {
            color_block(b, out, true);
        }),
        format::DXT5 => blocks(data, w, h, 16, |b, out| {
            color_block(&b[8..], out, false);
            alpha_block(&b[..8], out);
        }),
        _ => unreachable!(),
    })
}

/// Run `decode_block` over every 4x4 block and place its pixels.
fn blocks(
    data: &[u8],
    w: usize,
    h: usize,
    block_size: usize,
    decode_block: impl Fn(&[u8], &mut [[u8; 4]; 16]),
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let bw = w.div_ceil(4);
    for (i, block) in data.chunks_exact(block_size).enumerate() {
        let (bx, by) = (i % bw * 4, i / bw * 4);
        let mut px = [[0u8; 4]; 16];
        decode_block(block, &mut px);
        for (j, p) in px.iter().enumerate() {
            let (x, y) = (bx + j % 4, by + j / 4);
            if x < w && y < h {
                out[(y * w + x) * 4..][..4].copy_from_slice(p);
            }
        }
    }
    out
}

fn rgb565(c: u16) -> [u8; 3] {
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
    let mix = |a: u8, b: u8, wa: u16, wb: u16| ((a as u16 * wa + b as u16 * wb) / (wa + wb)) as u8;
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
    let (a0, a1) = (b[0] as u16, b[1] as u16);
    let mut palette = [0u8; 8];
    palette[0] = a0 as u8;
    palette[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7 {
            palette[i + 1] = (((7 - i as u16) * a0 + i as u16 * a1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            palette[i + 1] = (((5 - i as u16) * a0 + i as u16 * a1) / 5) as u8;
        }
        palette[6] = 0;
        palette[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &byte) in b[2..8].iter().enumerate() {
        bits |= (byte as u64) << (8 * i);
    }
    for (i, px) in out.iter_mut().enumerate() {
        px[3] = palette[(bits >> (i * 3)) as usize & 7];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dxt1_block_endpoints_and_transparency() {
        // c0 = pure red, c1 = pure blue, c0 > c1: four opaque colours.
        let red = 0xf800u16.to_le_bytes();
        let blue = 0x001fu16.to_le_bytes();
        // Indices: pixel 0 -> c0, pixel 1 -> c1, pixel 2 -> 2/3 c0 + 1/3 c1, rest 0.
        let block = [red[0], red[1], blue[0], blue[1], 0b10_01_00, 0, 0, 0];
        let out = decode(format::DXT1, 4, 4, &block).unwrap();
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 0, 255, 255]);
        assert_eq!(&out[8..12], &[170, 0, 85, 255]);

        // c0 <= c1 in DXT1: index 3 is transparent black.
        let block = [blue[0], blue[1], red[0], red[1], 0b11, 0, 0, 0];
        let out = decode(format::DXT1, 4, 4, &block).unwrap();
        assert_eq!(&out[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn test_dxt5_alpha_palette() {
        // a0 = 255, a1 = 0 (eight-value ramp); pixel 0 -> a0, pixel 1 -> a1, pixel 2 -> index 2.
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        let bits: u64 = 1 << 3 | 2 << 6;
        block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
        let out = decode(format::DXT5, 4, 4, &block).unwrap();
        assert_eq!(out[3], 255);
        assert_eq!(out[7], 0);
        assert_eq!(out[11], (6 * 255 / 7) as u8);
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
        assert!(decode(9999, 4, 4, &[0; 64]).is_err());
    }
}
