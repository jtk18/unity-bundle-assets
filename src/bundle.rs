//! Unity asset bundles (`UnityFS`): a header, a table of compressed blocks, and a directory of
//! the files stored in the decompressed block stream: usually one serialized file (`CAB-...`)
//! and the `.resS` / `.resource` stream files its textures and audio point into.
//!
//! The layout follows UnityPy's `BundleFile` reader (MIT, see NOTICE).

use crate::reader::Reader;
use crate::{version_numbers, Error, Limits, Result};

const SIGNATURE: &[u8] = b"UnityFS\0";

mod flags {
    pub const COMPRESSION_MASK: u32 = 0x3f;
    pub const BLOCKS_INFO_AT_END: u32 = 0x80;
    /// Only in the newer flag set; older engines used this bit for encryption.
    pub const BLOCK_INFO_NEEDS_PADDING: u32 = 0x200;
    pub const ENCRYPTION_OLD: u32 = 0x200;
    pub const ENCRYPTION_NEW: u32 = 0x1400;
    /// A directory entry holding a serialized file (rather than a stream file).
    pub const NODE_SERIALIZED: u32 = 0x4;
}

/// LZ4 cannot expand more than this: a match costs at least one input byte per 255 output.
const LZ4_MAX_RATIO: usize = 255;

/// One file inside a bundle.
#[derive(Debug, Clone)]
pub struct Entry {
    path: String,
    flags: u32,
    offset: usize,
    size: usize,
}

impl Entry {
    /// The file's name in the bundle's directory, e.g. `CAB-<hash>` or `CAB-<hash>.resS`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory flags; bit 2 marks a serialized file.
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// Size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether this entry is a serialized file rather than a stream file.
    pub fn is_serialized(&self) -> bool {
        self.flags & flags::NODE_SERIALIZED != 0
    }
}

/// A parsed bundle. Holds the whole decompressed block stream.
pub struct Bundle {
    format: u32,
    unity_version: String,
    unity_revision: String,
    entries: Vec<Entry>,
    data: Vec<u8>,
    limits: Limits,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("format", &self.format)
            .field("unity_revision", &self.unity_revision)
            .field("entries", &self.entries)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// Whether `data` starts like an asset bundle.
pub fn is_bundle(data: &[u8]) -> bool {
    data.starts_with(SIGNATURE)
}

impl Bundle {
    /// Read and parse the bundle at `path`, with the default [`Limits`].
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Bundle> {
        let limits = Limits::default();
        Bundle::parse_with(&crate::export::read_file(path.as_ref(), &limits)?, limits)
    }

    /// Parse a bundle from its bytes, decompressing every block, with the default [`Limits`].
    pub fn parse(file: &[u8]) -> Result<Bundle> {
        Bundle::parse_with(file, Limits::default())
    }

    /// Parse a bundle from its bytes under `limits`.
    pub fn parse_with(file: &[u8], limits: Limits) -> Result<Bundle> {
        if !is_bundle(file) {
            for other in ["UnityWeb", "UnityRaw", "UnityArchive"] {
                if file.starts_with(other.as_bytes()) {
                    return Err(Error::Unsupported(format!("{other} bundle container")));
                }
            }
            return Err(Error::NotUnity("no UnityFS signature".into()));
        }
        let mut r = Reader::new(file, true);
        r.cstr()?;
        let format = r.u32()?;
        let unity_version = r.cstr()?;
        let unity_revision = r.cstr()?;
        let _size = r.i64()?;
        let info_compressed = r.u32()? as usize;
        let info_size = r.u32()? as usize;
        let flags = r.u32()?;

        let revision = version_numbers(&unity_revision);
        if revision == [0, 0, 0] && flags & flags::BLOCK_INFO_NEEDS_PADDING != 0 {
            return Err(Error::Unsupported(format!(
                "bundle flag 0x200 with engine version {unity_revision:?}: it means padding \
                 after 2020.3.34 and encryption before, and the version is stripped"
            )));
        }
        // Engines before these used 0x200 for encryption; from them on it means padding.
        let new_flags = !(revision < [2020, 0, 0]
            || (revision[0] == 2020 && revision < [2020, 3, 34])
            || (revision[0] == 2021 && revision < [2021, 3, 2])
            || (revision[0] == 2022 && revision < [2022, 1, 1]));
        let encrypted = if new_flags {
            flags::ENCRYPTION_NEW
        } else {
            flags::ENCRYPTION_OLD
        };
        if flags & encrypted != 0 {
            return Err(Error::Encrypted);
        }
        if format >= 7 || (revision[0] == 2019 && revision >= [2019, 4, 15]) {
            r.align(16);
        }

        let budget = limits.max_decompressed;
        check_limit("bundle directory size", info_size as u64, budget)?;
        let info_bytes = if flags & flags::BLOCKS_INFO_AT_END != 0 {
            let start = file
                .len()
                .checked_sub(info_compressed)
                .ok_or(Error::Truncated(file.len()))?;
            &file[start..]
        } else {
            r.take(info_compressed)?
        };
        let mut info = Vec::new();
        decompress_into(
            &mut info,
            info_bytes,
            info_size,
            flags & flags::COMPRESSION_MASK,
        )?;
        let mut ir = Reader::new(&info, true);
        ir.skip(16)?; // hash of the uncompressed data
        let block_count = ir.len(10)?;
        let mut blocks = Vec::with_capacity(block_count);
        let mut total: u64 = 0;
        for _ in 0..block_count {
            let size = ir.u32()?;
            let compressed = ir.u32()?;
            let block_flags = ir.u16()?;
            total += size as u64;
            blocks.push((size as usize, compressed as usize, block_flags as u32));
        }
        check_limit("bundle decompressed size", total + info_size as u64, budget)?;
        let entry_count = ir.len(21)?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let offset = ir.i64()?;
            let size = ir.i64()?;
            let flags = ir.u32()?;
            let path = ir.cstr()?;
            let (Ok(offset), Ok(size)) = (usize::try_from(offset), usize::try_from(size)) else {
                return Err(Error::Invalid(format!("entry {path} at {offset}+{size}")));
            };
            if offset
                .checked_add(size)
                .is_none_or(|end| end as u64 > total)
            {
                return Err(Error::Invalid(format!("entry {path} runs past the bundle")));
            }
            entries.push(Entry {
                path,
                flags,
                offset,
                size,
            });
        }

        if new_flags && flags & flags::BLOCK_INFO_NEEDS_PADDING != 0 {
            r.align(16);
        }
        let mut data = Vec::new();
        for (size, compressed, block_flags) in blocks {
            let block = r.take(compressed)?;
            decompress_into(
                &mut data,
                block,
                size,
                block_flags & flags::COMPRESSION_MASK,
            )?;
        }

        Ok(Bundle {
            format,
            unity_version,
            unity_revision,
            entries,
            data,
            limits,
        })
    }

    /// Container format version (6 for Unity 5.x-2019.3, 7 from 2019.4, 8 from 2022).
    pub fn format(&self) -> u32 {
        self.format
    }

    /// The player version string, e.g. `5.x.x`.
    pub fn unity_version(&self) -> &str {
        &self.unity_version
    }

    /// The engine release that built the bundle, e.g. `2018.4.36f1`.
    pub fn unity_revision(&self) -> &str {
        &self.unity_revision
    }

    /// Every file in the bundle, in directory order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The limits the bundle was parsed under.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// One entry's bytes, or `None` for an entry from another bundle that does not fit this
    /// one.
    pub fn bytes(&self, entry: &Entry) -> Option<&[u8]> {
        self.data
            .get(entry.offset..entry.offset.checked_add(entry.size)?)
    }

    /// The entry with this path, or else the first with this file name (stream paths look
    /// like `archive:/CAB-.../CAB-....resS`; the directory lists `CAB-....resS`).
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.entries
            .iter()
            .find(|e| e.path == path)
            .or_else(|| self.entries.iter().find(|e| e.path == name))
    }

    pub(crate) fn data_ref(&self) -> &[u8] {
        &self.data
    }

    /// The byte range of an entry that [`Bundle::bytes`] has already accepted.
    pub(crate) fn range_of(&self, entry: &Entry) -> std::ops::Range<usize> {
        entry.offset..entry.offset + entry.size
    }

    /// The entries holding serialized files.
    pub fn serialized_files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_serialized())
    }
}

fn check_limit(what: &'static str, value: u64, limit: u64) -> Result<()> {
    if value > limit {
        return Err(Error::LimitExceeded { what, value, limit });
    }
    Ok(())
}

/// Append one block, decompressed, to `out`. The block must produce exactly `size` bytes.
fn decompress_into(out: &mut Vec<u8>, data: &[u8], size: usize, compression: u32) -> Result<()> {
    match compression {
        0 => {
            if data.len() != size {
                return Err(Error::Invalid(format!(
                    "stored block is {} bytes, header says {size}",
                    data.len()
                )));
            }
            out.extend_from_slice(data);
            Ok(())
        }
        1 => lzma_into(out, data, size),
        2 | 3 => lz4_into(out, data, size),
        other => Err(Error::Unsupported(format!(
            "bundle compression {other} (supported: none, LZMA, LZ4, LZ4HC)"
        ))),
    }
}

/// Unity's LZMA: the 5-byte properties header, then the raw stream, with no size field. The
/// output grows as it is written, so a false `size` costs nothing up front.
fn lzma_into(out: &mut Vec<u8>, data: &[u8], size: usize) -> Result<()> {
    use lzma_rs::decompress::{Options, UnpackedSize};
    let options = Options {
        unpacked_size: UnpackedSize::UseProvided(Some(size as u64)),
        // The dictionary never needs to be larger than what the block produces, and that is
        // already held to the bundle's decompression limit.
        memlimit: Some(size.max(4096)),
        allow_incomplete: false,
    };
    let start = out.len();
    lzma_rs::lzma_decompress_with_options(&mut &data[..], out, &options)
        .map_err(|e| Error::Invalid(format!("LZMA block: {e}")))?;
    let produced = out.len() - start;
    if produced != size {
        return Err(Error::Invalid(format!(
            "LZMA block gave {produced} bytes, header says {size}"
        )));
    }
    Ok(())
}

/// An LZ4 block (not frame): sequences of literals and back-references. LZ4HC produces the
/// same format. Matches refer only to this block's own output.
fn lz4_into(out: &mut Vec<u8>, data: &[u8], size: usize) -> Result<()> {
    let bad = |what: &str| Error::Invalid(format!("LZ4 block: {what}"));
    if size > data.len().saturating_mul(LZ4_MAX_RATIO).saturating_add(16) {
        return Err(bad("claims more output than LZ4 can encode in its input"));
    }
    let base = out.len();
    let end = base + size;
    out.reserve(size);
    let mut i = 0;
    let length = |i: &mut usize, mut n: usize| -> Result<usize> {
        if n == 15 {
            loop {
                let b = *data.get(*i).ok_or_else(|| bad("truncated length"))?;
                *i += 1;
                n = n
                    .checked_add(b as usize)
                    .ok_or_else(|| bad("length overflows"))?;
                if b != 255 {
                    break;
                }
            }
        }
        Ok(n)
    };
    loop {
        let token = *data.get(i).ok_or_else(|| bad("truncated"))?;
        i += 1;
        let literals = length(&mut i, (token >> 4) as usize)?;
        let src = i
            .checked_add(literals)
            .and_then(|e| data.get(i..e))
            .ok_or_else(|| bad("literals run past the input"))?;
        if out.len() + literals > end {
            return Err(bad("output larger than the header says"));
        }
        out.extend_from_slice(src);
        i += literals;
        if i == data.len() {
            break;
        }
        let offset = u16::from_le_bytes(
            data.get(i..i + 2)
                .ok_or_else(|| bad("truncated offset"))?
                .try_into()
                .unwrap(),
        ) as usize;
        i += 2;
        let matched = length(&mut i, (token & 15) as usize)? + 4;
        if offset == 0 || offset > out.len() - base {
            return Err(bad("offset outside the output"));
        }
        if out.len() + matched > end {
            return Err(bad("output larger than the header says"));
        }
        let start = out.len() - offset;
        if offset >= matched {
            out.extend_from_within(start..start + matched);
        } else {
            // The match overlaps the bytes it is producing: copy one period at a time.
            let mut left = matched;
            while left > 0 {
                let n = left.min(offset);
                let from = out.len() - offset;
                out.extend_from_within(from..from + n);
                left -= n;
            }
        }
    }
    if out.len() != end {
        return Err(bad(&format!(
            "gave {} bytes, header says {size}",
            out.len() - base
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lz4(data: &[u8], size: usize) -> Result<Vec<u8>> {
        let mut out = vec![0xee; 3]; // output from earlier blocks, out of reach
        lz4_into(&mut out, data, size)?;
        Ok(out[3..].to_vec())
    }

    #[test]
    fn test_lz4_literals_and_overlapping_match() {
        // "abc" as literals, then a match of 6 at offset 3, then an empty last sequence.
        let block = [0x32, b'a', b'b', b'c', 3, 0, 0x00];
        assert_eq!(lz4(&block, 9).unwrap(), b"abcabcabc");
        assert!(lz4(&block, 8).is_err());
        assert!(lz4(&block[..6], 9).is_err()); // no final literals-only sequence
        assert!(lz4(&[0x10, b'a', 5, 0], 10).is_err()); // offset past the output
    }

    #[test]
    fn test_lz4_offset_one_run() {
        // "a", then a match of 19 at offset 1 (a run), then an empty last sequence.
        let block = [0x1f, b'a', 1, 0, 0, 0x00];
        assert_eq!(lz4(&block, 20).unwrap(), vec![b'a'; 20]);
    }

    #[test]
    fn test_lz4_long_match_length() {
        // "ab", match offset 2 of 4 + 15 + 255 + 10 = 284 bytes, then empty last sequence.
        let block = [0x2f, b'a', b'b', 2, 0, 255, 10, 0x00];
        let out = lz4(&block, 286).unwrap();
        assert_eq!(out.len(), 286);
        assert!(out.chunks(2).all(|c| c == b"ab"));
    }

    #[test]
    fn test_lz4_long_literal_length() {
        // 20 literals: token nibble 15, then 5.
        let mut block = vec![0xf0, 5];
        block.extend(0..20u8);
        assert_eq!(lz4(&block, 20).unwrap(), (0..20u8).collect::<Vec<_>>());
    }

    #[test]
    fn test_lz4_refuses_offset_zero_and_earlier_blocks() {
        assert!(lz4(&[0x10, b'a', 0, 0, 0x00], 5).is_err()); // offset 0
                                                             // Offset 2 would reach the earlier block's bytes; this block has only one.
        assert!(lz4(&[0x10, b'a', 2, 0, 0x00], 5).is_err());
    }

    #[test]
    fn test_lz4_refuses_impossible_ratio() {
        assert!(lz4(&[0x00], 1 << 20).is_err());
    }

    #[test]
    fn test_not_a_bundle() {
        assert!(matches!(
            Bundle::parse(b"UnityWeb\0\0\0\0\x06"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Bundle::parse(b"\x89PNG\r\n"),
            Err(Error::NotUnity(_))
        ));
    }
}
