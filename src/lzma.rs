//! An LZMA decoder for Unity's bundle blocks, written from Igor Pavlov's LZMA specification
//! (`lzma-specification.txt` in the LZMA SDK, public domain).
//!
//! A Unity block is decoded whole into a buffer already sized to its output, so the output
//! doubles as the dictionary: a match is a copy within that buffer, and there is no window to
//! wrap. Decoding stops when the buffer is full; an end marker before that is an error, and
//! anything after it is not read.

/// Why a block did not decode.
pub(crate) struct Fault {
    pub what: String,
}

const NUM_STATES: usize = 12;
const POS_STATES_MAX: usize = 16;
const LEN_STATES: usize = 4;
const END_POS_MODEL: u32 = 14;
const FULL_DISTANCES: usize = 128;
const ALIGN_BITS: u32 = 4;
const MATCH_MIN_LEN: usize = 2;
const PROB_INIT: u16 = 1024;

/// A header's literal and position parameters: `lc`, `lp`, `pb`, and the dictionary size.
pub(crate) struct Props {
    lc: u32,
    lp: u32,
    pb: u32,
    dictionary: u32,
}

impl Props {
    /// The 5-byte header; `None` when its parameters are outside LZMA's bounds.
    pub(crate) fn parse(header: [u8; 5]) -> Option<Self> {
        let d = u32::from(header[0]);
        let (lc, lp, pb) = (d % 9, d / 9 % 5, d / 45);
        if pb > 4 || lc + lp > 4 {
            return None;
        }
        let dictionary = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
        Some(Self {
            lc,
            lp,
            pb,
            dictionary: dictionary.max(1 << 12),
        })
    }
}

struct RangeDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

// The per-bit functions are forced inline: with plain `#[inline]` they stayed calls, and
// parsing 100 real bundles took 2.55 s against 2.06 s.
#[allow(clippy::inline_always)]
impl<'a> RangeDecoder<'a> {
    /// The first byte is always 0 from an encoder and carries nothing; like lzma-rs (which this
    /// crate used before), a nonzero one is not refused, nor a first code at the top of the
    /// range, which the specification lets a decoder flag.
    fn new(data: &'a [u8]) -> Option<Self> {
        let [_, a, b, c, d, ..] = *data else {
            return None;
        };
        let code = u32::from_be_bytes([a, b, c, d]);
        Some(Self {
            data,
            pos: 5,
            range: u32::MAX,
            code,
        })
    }

    /// The next input byte. Past the end the stream is truncated; the decoder reads zeros and
    /// `truncated` reports it, so the hot path carries no early return.
    #[inline(always)]
    fn next(&mut self) -> u8 {
        let b = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        b
    }

    const fn truncated(&self) -> bool {
        self.pos > self.data.len()
    }

    #[inline(always)]
    fn normalize(&mut self) {
        if self.range < 1 << 24 {
            self.range <<= 8;
            self.code = (self.code << 8) | u32::from(self.next());
        }
    }

    #[inline(always)]
    fn bit(&mut self, prob: &mut u16) -> u32 {
        let bound = (self.range >> 11) * u32::from(*prob);
        let bit = if self.code < bound {
            *prob += (2048 - *prob) >> 5;
            self.range = bound;
            0
        } else {
            *prob -= *prob >> 5;
            self.code -= bound;
            self.range -= bound;
            1
        };
        self.normalize();
        bit
    }

    /// `bits` bits at even odds. `false` in the second place when the stream is corrupt.
    fn direct(&mut self, bits: u32) -> (u32, bool) {
        let mut res = 0u32;
        let mut ok = true;
        for _ in 0..bits {
            self.range >>= 1;
            self.code = self.code.wrapping_sub(self.range);
            let t = 0u32.wrapping_sub(self.code >> 31);
            self.code = self.code.wrapping_add(self.range & t);
            if self.code == self.range {
                ok = false;
            }
            self.normalize();
            res = (res << 1).wrapping_add(t.wrapping_add(1));
        }
        (res, ok)
    }

    /// A `bits`-bit symbol, most significant bit first, from the tree at `probs`.
    #[inline(always)]
    fn tree(&mut self, probs: &mut [u16], bits: u32) -> usize {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) + self.bit(&mut probs[m]) as usize;
        }
        m - (1 << bits)
    }

    /// A `bits`-bit symbol, least significant bit first.
    #[inline(always)]
    fn reverse(&mut self, probs: &mut [u16], bits: u32) -> u32 {
        let (mut m, mut symbol) = (1usize, 0u32);
        for i in 0..bits {
            let bit = self.bit(&mut probs[m]);
            m = (m << 1) + bit as usize;
            symbol |= bit << i;
        }
        symbol
    }
}

struct LenDecoder {
    choice: u16,
    choice2: u16,
    low: [[u16; 8]; POS_STATES_MAX],
    mid: [[u16; 8]; POS_STATES_MAX],
    high: [u16; 256],
}

#[allow(clippy::inline_always)]
impl LenDecoder {
    const fn new() -> Self {
        Self {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: [[PROB_INIT; 8]; POS_STATES_MAX],
            mid: [[PROB_INIT; 8]; POS_STATES_MAX],
            high: [PROB_INIT; 256],
        }
    }

    #[inline(always)]
    fn decode(&mut self, rc: &mut RangeDecoder<'_>, pos_state: usize) -> usize {
        if rc.bit(&mut self.choice) == 0 {
            return rc.tree(&mut self.low[pos_state], 3);
        }
        if rc.bit(&mut self.choice2) == 0 {
            return 8 + rc.tree(&mut self.mid[pos_state], 3);
        }
        16 + rc.tree(&mut self.high, 8)
    }
}

/// Grow `out` to hold at least `need` bytes: to twice what the block has made so far (64 KiB
/// at least), never past `end`. Memory is then taken as output is made, not as the block
/// claims: a block that claims 1 GiB and fails early touches little of it.
pub(crate) fn grow(out: &mut Vec<u8>, start: usize, end: usize, need: usize) {
    let made = out.len() - start;
    out.resize(need.max(out.len() + made.max(1 << 16)).min(end), 0);
}

/// Decode `data` (the stream after the 5-byte header), appending `size` bytes to `out`. On
/// failure, `out` holds what the block made before the fault.
pub(crate) fn decode(
    props: &Props,
    data: &[u8],
    out: &mut Vec<u8>,
    size: usize,
) -> Result<(), Fault> {
    let start = out.len();
    let end = start + size;
    let mut o = start;
    let fail = |out: &mut Vec<u8>, o: usize, what: &str| {
        out.truncate(o);
        Err(Fault {
            what: what.to_owned(),
        })
    };
    let Some(mut rc) = RangeDecoder::new(data) else {
        return fail(out, o, "shorter than its range coder's first five bytes");
    };
    let (lc, lp) = (props.lc, props.lp);
    let pb_mask = (1usize << props.pb) - 1;
    let lp_mask = (1usize << lp) - 1;
    let dictionary = props.dictionary as usize;

    let mut literals = vec![PROB_INIT; 0x300 << (lc + lp)];
    let mut is_match = [PROB_INIT; NUM_STATES * POS_STATES_MAX];
    let mut is_rep = [PROB_INIT; NUM_STATES];
    let mut is_rep_g0 = [PROB_INIT; NUM_STATES];
    let mut is_rep_g1 = [PROB_INIT; NUM_STATES];
    let mut is_rep_g2 = [PROB_INIT; NUM_STATES];
    let mut is_rep0_long = [PROB_INIT; NUM_STATES * POS_STATES_MAX];
    let mut pos_slot = [[PROB_INIT; 64]; LEN_STATES];
    let mut pos_special = [PROB_INIT; 1 + FULL_DISTANCES - END_POS_MODEL as usize];
    let mut align = [PROB_INIT; 1 << ALIGN_BITS];
    let mut len_decoder = LenDecoder::new();
    let mut rep_len_decoder = LenDecoder::new();

    let mut state = 0usize;
    // Distances less one, as the specification keeps them.
    let (mut rep0, mut rep1, mut rep2, mut rep3) = (0usize, 0usize, 0usize, 0usize);

    while o < end {
        if rc.truncated() {
            return fail(out, o, "truncated");
        }
        if o == out.len() {
            grow(out, start, end, o + 1);
        }
        let pos_state = o & pb_mask;
        if rc.bit(&mut is_match[(state << 4) + pos_state]) == 0 {
            let prev = if o == start { 0 } else { out[o - 1] };
            let base = 0x300 * (((o & lp_mask) << lc) + (usize::from(prev) >> (8 - lc)));
            let table = &mut literals[base..base + 0x300];
            let mut symbol = 1usize;
            if state >= 7 {
                // After a match, the byte at the last distance guides the first bits.
                if rep0 >= o - start {
                    return fail(out, o, "distance outside the output");
                }
                let m = out[o - rep0 - 1];
                let mut match_byte = usize::from(m);
                while symbol < 0x100 {
                    let match_bit = (match_byte >> 7) & 1;
                    match_byte <<= 1;
                    let bit = rc.bit(&mut table[((1 + match_bit) << 8) + symbol]) as usize;
                    symbol = (symbol << 1) | bit;
                    if match_bit != bit {
                        break;
                    }
                }
            }
            while symbol < 0x100 {
                symbol = (symbol << 1) | rc.bit(&mut table[symbol]) as usize;
            }
            out[o] = symbol as u8;
            o += 1;
            state = if state < 4 {
                0
            } else if state < 10 {
                state - 3
            } else {
                state - 6
            };
            continue;
        }
        let len;
        if rc.bit(&mut is_rep[state]) != 0 {
            if o == start {
                return fail(out, o, "repeat before any output");
            }
            if rc.bit(&mut is_rep_g0[state]) == 0 {
                if rc.bit(&mut is_rep0_long[(state << 4) + pos_state]) == 0 {
                    // A single byte from the last distance.
                    state = if state < 7 { 9 } else { 11 };
                    if rep0 >= o - start {
                        return fail(out, o, "distance outside the output");
                    }
                    out[o] = out[o - rep0 - 1];
                    o += 1;
                    continue;
                }
            } else {
                let dist;
                if rc.bit(&mut is_rep_g1[state]) == 0 {
                    dist = rep1;
                } else {
                    if rc.bit(&mut is_rep_g2[state]) == 0 {
                        dist = rep2;
                    } else {
                        dist = rep3;
                        rep3 = rep2;
                    }
                    rep2 = rep1;
                }
                rep1 = rep0;
                rep0 = dist;
            }
            len = rep_len_decoder.decode(&mut rc, pos_state);
            state = if state < 7 { 8 } else { 11 };
        } else {
            rep3 = rep2;
            rep2 = rep1;
            rep1 = rep0;
            len = len_decoder.decode(&mut rc, pos_state);
            state = if state < 7 { 7 } else { 10 };
            // The distance, from a slot and the bits under it.
            let slot = rc.tree(&mut pos_slot[len.min(LEN_STATES - 1)], 6) as u32;
            rep0 = if slot < 4 {
                slot as usize
            } else {
                let direct = (slot >> 1) - 1;
                let base = (2 | (slot & 1)) << direct;
                if slot < END_POS_MODEL {
                    let at = (base - slot) as usize;
                    // The tree for this slot starts at `at`; its index 1 is `at + 1`.
                    base as usize + rc.reverse(&mut pos_special[at..], direct) as usize
                } else {
                    let (high, ok) = rc.direct(direct - ALIGN_BITS);
                    if !ok {
                        return fail(out, o, "corrupt distance");
                    }
                    let low = rc.reverse(&mut align, ALIGN_BITS);
                    // Wraps to u32::MAX, the end marker, as the specification's does.
                    (base.wrapping_add(high << ALIGN_BITS).wrapping_add(low)) as usize
                }
            };
            if rep0 == u32::MAX as usize {
                return fail(out, o, "end marker before the block's size");
            }
            if rep0 >= dictionary {
                return fail(
                    out,
                    o,
                    &format!(
                        "distance {} past the dictionary's {dictionary} bytes",
                        rep0 as u64 + 1
                    ),
                );
            }
        }
        let len = len + MATCH_MIN_LEN;
        let dist = rep0 + 1;
        if dist > o - start {
            let made = o - start;
            return fail(
                out,
                o,
                &format!("distance {dist} with {made} bytes of output"),
            );
        }
        if len > end - o {
            return fail(out, o, "output larger than the header says");
        }
        if o + len > out.len() {
            grow(out, start, end, o + len);
        }
        let from = o - dist;
        if dist >= len {
            out.copy_within(from..from + len, o);
        } else {
            // Overlapping: the output repeats with period `dist`, so copy doubling spans.
            let mut done = 0;
            while done < len {
                let n = (len - done).min(o + done - from);
                out.copy_within(from..from + n, o + done);
                done += n;
            }
        }
        o += len;
    }
    if rc.truncated() {
        return fail(out, o, "truncated");
    }
    Ok(())
}
