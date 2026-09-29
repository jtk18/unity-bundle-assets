//! BC7 blocks, from the BPTC specification (the D3D11 functional specification and
//! `EXT_texture_compression_bptc`). The partition and anchor tables are the specification's,
//! in the packed layout texture2ddecoder uses (see NOTICE).

/// Per mode: subsets, partition bits, rotation bits, index-selection bits, colour bits,
/// alpha bits, a p-bit per endpoint, a p-bit per subset, index bits, second index bits.
const MODES: [[u8; 10]; 8] = [
    [3, 4, 0, 0, 4, 0, 1, 0, 3, 0],
    [2, 6, 0, 0, 6, 0, 0, 1, 3, 0],
    [3, 6, 0, 0, 5, 0, 0, 0, 2, 0],
    [2, 6, 0, 0, 7, 0, 1, 0, 2, 0],
    [1, 0, 2, 1, 5, 6, 0, 0, 2, 3],
    [1, 0, 2, 0, 7, 8, 0, 0, 2, 2],
    [1, 0, 0, 0, 7, 7, 1, 0, 4, 0],
    [2, 6, 0, 0, 5, 5, 1, 0, 2, 0],
];

const WEIGHTS2: [u16; 4] = [0, 21, 43, 64];
const WEIGHTS3: [u16; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const WEIGHTS4: [u16; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

/// Two-subset partitions, a bit a pixel (pixel 0 lowest).
const PARTITION2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800,
    0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100, 0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce,
    0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc,
    0x3c3c, 0x55aa, 0x9696, 0xa55a, 0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660,
    0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336, 0x9cc6, 0x817e, 0xe718,
    0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// Three-subset partitions, two bits a pixel (pixel 0 lowest).
const PARTITION3: [u32; 64] = [
    0xaa68_5050,
    0x6a5a_5040,
    0x5a5a_4200,
    0x5450_a0a8,
    0xa5a5_0000,
    0xa0a0_5050,
    0x5555_a0a0,
    0x5a5a_5050,
    0xaa55_0000,
    0xaa55_5500,
    0xaaaa_5500,
    0x9090_9090,
    0x9494_9494,
    0xa4a4_a4a4,
    0xa9a5_9450,
    0x2a0a_4250,
    0xa594_5040,
    0x0a42_5054,
    0xa5a5_a500,
    0x55a0_a0a0,
    0xa8a8_5454,
    0x6a6a_4040,
    0xa4a4_5000,
    0x1a1a_0500,
    0x0050_a4a4,
    0xaaa5_9090,
    0x1469_6914,
    0x6969_1400,
    0xa085_85a0,
    0xaa82_1414,
    0x50a4_a450,
    0x6a5a_0200,
    0xa9a5_8000,
    0x5090_a0a8,
    0xa8a0_9050,
    0x2424_2424,
    0x00aa_5500,
    0x2492_4924,
    0x2449_9224,
    0x50a5_0a50,
    0x500a_a550,
    0xaaaa_4444,
    0x6666_0000,
    0xa5a0_a5a0,
    0x50a0_50a0,
    0x6928_6928,
    0x44aa_aa44,
    0x6666_6600,
    0xaa44_4444,
    0x54a8_54a8,
    0x9580_9580,
    0x9696_9600,
    0xa854_54a8,
    0x8095_9580,
    0xaa14_1414,
    0x9696_0000,
    0xaaaa_1414,
    0xa050_50a0,
    0xa0a5_a5a0,
    0x9600_0000,
    0x4080_4080,
    0xa9a8_a9a8,
    0xaaaa_aa44,
    0x2a4a_5254,
];

/// Where the second subset's anchor pixel is, by two-subset partition.
const ANCHOR2: [u8; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8,
    2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2,
    2, 15, 15, 15, 15, 15, 2, 2, 15,
];

/// Where the second and third subsets' anchor pixels are, by three-subset partition.
const ANCHOR3: [[u8; 64]; 2] = [
    [
        3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6,
        8, 5, 15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8,
        5, 10, 5, 10, 8, 13, 15, 12, 3, 3,
    ],
    [
        15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3,
        15, 6, 10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15,
        15, 15, 15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
    ],
];
struct Bits {
    bits: u128,
    pos: u32,
}

impl Bits {
    fn read(&mut self, n: u8) -> u8 {
        // Past the block's 128 bits there is nothing left: zeros.
        let v = self.bits.checked_shr(self.pos).unwrap_or(0) as u32 & ((1u32 << n) - 1);
        self.pos += u32::from(n);
        v as u8
    }
}

/// An endpoint channel of `bits` bits widened to eight, its top bits repeated below. Eight
/// bits (mode 5's alpha) are already a byte.
const fn expand(v: u8, bits: u8) -> u8 {
    if bits >= 8 {
        return v;
    }
    let v = v << (8 - bits);
    v | (v >> bits)
}

const fn weights(bits: u8) -> &'static [u16] {
    match bits {
        2 => &WEIGHTS2,
        3 => &WEIGHTS3,
        _ => &WEIGHTS4,
    }
}

const fn interpolate(e0: u8, e1: u8, w: u16) -> u8 {
    (((64 - w) * e0 as u16 + w * e1 as u16 + 32) >> 6) as u8
}

/// One 16-byte block to sixteen RGBA pixels, row by row. A block in the reserved mode (its
/// first byte 0) decodes to transparent black, as the specification says.
pub(crate) fn block(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let bytes: [u8; 16] = b.try_into().unwrap_or([0; 16]);
    let mut r = Bits {
        bits: u128::from_le_bytes(bytes),
        pos: 0,
    };
    let mode = bytes[0].trailing_zeros() as usize;
    let Some(&[ns, pb, rb, isb, cb, ab, epb, spb, ib, ib2]) = MODES.get(mode) else {
        *out = [[0; 4]; 16];
        return;
    };
    r.pos = mode as u32 + 1;
    let partition = usize::from(r.read(pb));
    let rotation = r.read(rb);
    let selection = r.read(isb);

    let ends = usize::from(ns) * 2;
    let mut ep = [[0u8; 4]; 6];
    for c in 0..3 {
        for e in ep.iter_mut().take(ends) {
            e[c] = r.read(cb);
        }
    }
    if ab > 0 {
        for e in ep.iter_mut().take(ends) {
            e[3] = r.read(ab);
        }
    }
    let channels = if ab > 0 { 4 } else { 3 };
    let pbit = epb | spb;
    if epb == 1 {
        for e in ep.iter_mut().take(ends) {
            let p = r.read(1);
            e[..channels].iter_mut().for_each(|v| *v = (*v << 1) | p);
        }
    } else if spb == 1 {
        for s in 0..usize::from(ns) {
            let p = r.read(1);
            for e in &mut ep[s * 2..s * 2 + 2] {
                e[..channels].iter_mut().for_each(|v| *v = (*v << 1) | p);
            }
        }
    }
    let (cbits, abits) = (cb + pbit, ab + pbit);
    for e in ep.iter_mut().take(ends) {
        for v in &mut e[..3] {
            *v = expand(*v, cbits);
        }
        e[3] = if ab > 0 { expand(e[3], abits) } else { 255 };
    }

    let subset = |i: usize| -> usize {
        match ns {
            2 => usize::from((PARTITION2[partition] >> i) & 1 == 1),
            3 => (PARTITION3[partition] >> (2 * i)) as usize & 3,
            _ => 0,
        }
    };
    let anchor = |i: usize| -> bool {
        i == 0
            || match ns {
                2 => i == usize::from(ANCHOR2[partition]),
                3 => {
                    i == usize::from(ANCHOR3[0][partition])
                        || i == usize::from(ANCHOR3[1][partition])
                }
                _ => false,
            }
    };
    let mut first = [0u8; 16];
    for (i, v) in first.iter_mut().enumerate() {
        *v = r.read(ib - u8::from(anchor(i)));
    }
    let mut second = [0u8; 16];
    if ib2 > 0 {
        for (i, v) in second.iter_mut().enumerate() {
            *v = r.read(ib2 - u8::from(i == 0));
        }
    }
    // Which index set, of how many bits, gives colour and which alpha.
    let ((ci, cw), (ai, aw)) = if ib2 == 0 {
        ((&first, ib), (&first, ib))
    } else if selection == 0 {
        ((&first, ib), (&second, ib2))
    } else {
        ((&second, ib2), (&first, ib))
    };
    let (cw, aw) = (weights(cw), weights(aw));
    for (i, px) in out.iter_mut().enumerate() {
        let sub = subset(i);
        let (e0, e1) = (ep[sub * 2], ep[sub * 2 + 1]);
        let colour = cw[usize::from(ci[i])];
        let alpha = aw[usize::from(ai[i])];
        let mut p = [
            interpolate(e0[0], e1[0], colour),
            interpolate(e0[1], e1[1], colour),
            interpolate(e0[2], e1[2], colour),
            interpolate(e0[3], e1[3], alpha),
        ];
        match rotation {
            1 => p.swap(3, 0),
            2 => p.swap(3, 1),
            3 => p.swap(3, 2),
            _ => {}
        }
        *px = p;
    }
}
