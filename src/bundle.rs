//! Unity asset bundles (`UnityFS`): a header, a table of compressed blocks, and a directory of
//! the files stored in the decompressed block stream: usually one serialized file (`CAB-...`)
//! and the `.resS` / `.resource` stream files its textures and audio point into.
//!
//! The layout follows `UnityPy`'s `BundleFile` reader (MIT, see NOTICE).

use crate::reader::Reader;
use crate::{check_version_string, quoted, Error, LimitKind, Limits, Result, Shared, Version};

use std::sync::Arc;

use std::collections::HashMap;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: String,
    flags: u32,
    pub(crate) offset: usize,
    size: usize,
}

impl Entry {
    /// The file's name in the bundle's directory, e.g. `CAB-<hash>` or `CAB-<hash>.resS`.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Directory flags; bit 2 marks a serialized file.
    #[must_use]
    pub const fn flags(&self) -> u32 {
        self.flags
    }

    /// Size in bytes.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Whether this entry is a serialized file rather than a stream file.
    #[must_use]
    pub const fn is_serialized(&self) -> bool {
        self.flags & flags::NODE_SERIALIZED != 0
    }
}

/// A parsed bundle. Holds the whole decompressed block stream.
pub struct Bundle {
    format: u32,
    unity_version: String,
    unity_revision: String,
    entries: Vec<Entry>,
    /// Entry index by path (no two entries share one), and by the file name after the last
    /// `/` (`None` when two entries share it: the name alone names neither).
    by_path: HashMap<String, usize>,
    by_name: HashMap<String, Option<usize>>,
    data: Vec<u8>,
    limits: Limits,
    /// Work done and stream ranges read, shared by every [`crate::Assets`] opened from here.
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Bundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bundle")
            .field("format", &self.format)
            .field("unity_version", &self.unity_version)
            .field("unity_revision", &self.unity_revision)
            .field("entries", &self.entries.len())
            .field("bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

/// Whether `data` starts like an asset bundle.
#[must_use]
pub fn is_bundle(data: &[u8]) -> bool {
    data.starts_with(SIGNATURE)
}

impl Bundle {
    /// Read and parse the bundle at `path`, with the default [`Limits`].
    ///
    /// # Errors
    ///
    /// When the file cannot be read, is over a limit, or is not a bundle this crate reads.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::open_with(path, Limits::default())
    }

    /// [`Bundle::open`] under `limits`.
    ///
    /// # Errors
    ///
    /// As [`Bundle::open`].
    pub fn open_with(path: impl AsRef<std::path::Path>, limits: Limits) -> Result<Self> {
        let data = crate::file::read_limited(path.as_ref(), limits.max_file_size, check_head)?;
        Self::parse_with(&data, limits)
    }

    /// Parse a bundle from its bytes, decompressing every block, with the default [`Limits`].
    ///
    /// # Errors
    ///
    /// When the data is not a bundle this crate reads, or is over a limit.
    pub fn parse(file: &[u8]) -> Result<Self> {
        Self::parse_with(file, Limits::default())
    }

    /// Parse a bundle from its bytes under `limits`.
    ///
    /// # Errors
    ///
    /// As [`Bundle::parse`].
    pub fn parse_with(file: &[u8], limits: Limits) -> Result<Self> {
        let mut head = Reader::new(file, true);
        let Header {
            format,
            unity_version,
            unity_revision,
            declared_size,
        } = header(&mut head)?;
        // The bundle is the declared size; anything after it is not part of it.
        let size = declared_len(
            declared_size,
            head.pos() + REST_OF_HEADER,
            file.len() as u64,
        )?;
        let file = &file[..size as usize];
        let mut r = Reader::new(file, true);
        header(&mut r)?;
        let info_compressed = r.u32()? as usize;
        let info_size = r.u32()? as usize;
        let flags = r.u32()?;

        let revision = Version::parse(&unity_revision);
        if revision.stripped() {
            // Without the engine version the two flag sets cannot be told apart. Bits only
            // the newer set defines are encryption either way; 0x200 is ambiguous.
            if flags & (flags::ENCRYPTION_NEW & !flags::ENCRYPTION_OLD) != 0 {
                return Err(Error::Encrypted);
            }
            if flags & flags::BLOCK_INFO_NEEDS_PADDING != 0 {
                return Err(Error::Unsupported(format!(
                    "bundle flag 0x200 with engine version {unity_revision:?}: it means padding \
                     after 2020.3.34 and encryption before, and the version is stripped"
                )));
            }
        }
        let revision = revision.numbers;
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
        Error::limit(LimitKind::Decompressed, info_size as u64, budget)?;
        let info_bytes = if flags & flags::BLOCKS_INFO_AT_END != 0 {
            // The directory ends where the bundle does.
            let start = file
                .len()
                .checked_sub(info_compressed)
                .ok_or(Error::Truncated(file.len()))?;
            &file[start..]
        } else {
            r.take(info_compressed)?
        };
        if flags & flags::COMPRESSION_MASK == 1 {
            // The directory's LZMA working memory, as for the data blocks below.
            let (tables, dictionary) = lzma_memory(info_bytes, info_size)?;
            Error::limit(
                LimitKind::Decompressed,
                info_size as u64 + tables + dictionary,
                budget,
            )?;
        }
        let mut info = Vec::new();
        info.try_reserve_exact(info_size)
            .map_err(|_| Error::OutOfMemory {
                bytes: info_size as u64,
            })?;
        decompress_into(
            &mut info,
            info_bytes,
            info_size,
            flags & flags::COMPRESSION_MASK,
        )?;
        let mut ir = Reader::new(&info, true);
        ir.skip(16)?; // hash of the uncompressed data
        let block_count = ir.len(10)?;
        // The parsed directory costs more than its bytes; charge it at its in-memory size.
        let mut charged = info_size as u64
            + block_count as u64 * std::mem::size_of::<(usize, usize, u32)>() as u64;
        Error::limit(LimitKind::Decompressed, charged, budget)?;
        let mut blocks = Vec::with_capacity(block_count);
        let mut total: u64 = 0;
        for _ in 0..block_count {
            let size = ir.u32()?;
            let compressed = ir.u32()?;
            let block_flags = ir.u16()?;
            total += u64::from(size);
            blocks.push((size as usize, compressed as usize, u32::from(block_flags)));
        }
        charged += total;
        Error::limit(LimitKind::Decompressed, charged, budget)?;
        let entry_count = ir.len(21)?;
        // Each entry is held once and indexed twice (by path and by file name). A hash map's
        // table is rounded up to a power of two over 8/7 of its entries, up to about 2.3
        // buckets an entry with a control byte each; and each of the three copies of the path
        // carries the allocator's overhead, charged here at 32 bytes.
        let bucket = std::mem::size_of::<(String, usize)>() + 1;
        let per_entry = std::mem::size_of::<Entry>() + 2 * (bucket * 7 / 3) + 3 * 32;
        charged += entry_count as u64 * per_entry as u64;
        Error::limit(LimitKind::Decompressed, charged, budget)?;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let offset = ir.i64()?;
            let size = ir.i64()?;
            let entry_flags = ir.u32()?;
            let path = ir.cstr()?;
            charged += 3 * path.len() as u64;
            Error::limit(LimitKind::Decompressed, charged, budget)?;
            let (Ok(offset), Ok(size)) = (usize::try_from(offset), usize::try_from(size)) else {
                return Err(Error::Invalid(format!(
                    "entry {path} at {offset}+{size}",
                    path = quoted(&path)
                )));
            };
            if offset
                .checked_add(size)
                .is_none_or(|end| end as u64 > total)
            {
                return Err(Error::Invalid(format!(
                    "entry {path} runs past the bundle",
                    path = quoted(&path)
                )));
            }
            entries.push(Entry {
                path,
                flags: entry_flags,
                offset,
                size,
            });
        }

        // Unity never writes two entries over the same bytes; allowing it would let a small
        // bundle name one large file many times over.
        let mut spans: Vec<(usize, usize)> = entries
            .iter()
            .filter(|e| e.size > 0)
            .map(|e| (e.offset, e.offset + e.size))
            .collect();
        spans.sort_unstable();
        if spans.windows(2).any(|w| w[1].0 < w[0].1) {
            return Err(Error::Invalid("bundle entries overlap".into()));
        }
        drop(spans);

        if new_flags && flags & flags::BLOCK_INFO_NEEDS_PADDING != 0 {
            r.align(16);
        }
        // `total` is within the limit; reserve it once rather than grow by doubling.
        let mut data = Vec::new();
        data.try_reserve_exact(usize::try_from(total).unwrap_or(usize::MAX))
            .map_err(|_| Error::OutOfMemory { bytes: total })?;
        for (size, compressed, block_flags) in blocks {
            let block = r.take(compressed)?;
            if block_flags & flags::COMPRESSION_MASK == 1 {
                // LZMA's working memory is not part of its output: a table sized by the
                // header's lc and lp for every block, and a dictionary as large as the output
                // or the header's dictionary size, whichever is smaller. Held one block at a
                // time, and charged against what the output leaves of the limit; the tables
                // also add up, since many tiny blocks cost their setup many times over.
                let (tables, dictionary) = lzma_memory(block, size)?;
                charged += tables;
                Error::limit(LimitKind::Decompressed, charged + dictionary, budget)?;
            }
            decompress_into(
                &mut data,
                block,
                size,
                block_flags & flags::COMPRESSION_MASK,
            )?;
        }

        // One entry to a path: with two, which one a path names would depend on how it is
        // looked up. A file name two entries share names neither.
        let mut by_path = HashMap::with_capacity(entries.len());
        let mut by_name = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            if by_path.insert(e.path.clone(), i).is_some() {
                return Err(Error::Invalid(format!(
                    "bundle entry {} appears twice",
                    quoted(&e.path)
                )));
            }
            by_name
                .entry(file_name(&e.path).to_string())
                .and_modify(|v: &mut Option<usize>| *v = None)
                .or_insert(Some(i));
        }
        Ok(Self {
            format,
            unity_version,
            unity_revision,
            entries,
            by_path,
            by_name,
            data,
            limits,
            shared: Arc::new(Shared::default()),
        })
    }

    /// Container format version: 6 for Unity 5.x to 2019.4, then 7 and 8 in later releases
    /// (the release that introduced each is not pinned here).
    #[must_use]
    pub const fn format(&self) -> u32 {
        self.format
    }

    /// The player version string, e.g. `5.x.x`.
    #[must_use]
    pub fn unity_version(&self) -> &str {
        &self.unity_version
    }

    /// The engine release that built the bundle, e.g. `2018.4.36f1`.
    #[must_use]
    pub fn unity_revision(&self) -> &str {
        &self.unity_revision
    }

    /// Every file in the bundle, in directory order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The limits the bundle was parsed under.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// One entry's bytes, or `None` for an entry that does not fit this bundle. An [`Entry`]
    /// is not tied to the bundle it came from: one from another bundle that fits reads this
    /// bundle's bytes at its offsets.
    #[must_use]
    pub fn bytes(&self, entry: &Entry) -> Option<&[u8]> {
        self.data
            .get(entry.offset..entry.offset.checked_add(entry.size)?)
    }

    /// The entry with this path, or else the first with this file name (stream paths look
    /// like `archive:/CAB-.../CAB-....resS`; the directory lists `CAB-....resS`).
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        self.by_path
            .get(path)
            .or_else(|| self.by_name.get(file_name(path)).and_then(Option::as_ref))
            .map(|&i| &self.entries[i])
    }

    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.shared.clone()
    }

    pub(crate) fn data_ref(&self) -> &[u8] {
        &self.data
    }

    /// The byte range of an entry that [`Bundle::bytes`] has already accepted.
    pub(crate) const fn range_of(entry: &Entry) -> std::ops::Range<usize> {
        entry.offset..entry.offset + entry.size
    }

    /// The entries holding serialized files.
    pub fn serialized_files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_serialized())
    }
}

/// The part of a path after its last `/`.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
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

/// A bundle's header, up to its declared size.
struct Header {
    format: u32,
    unity_version: String,
    unity_revision: String,
    declared_size: i64,
}

fn header(r: &mut Reader<'_>) -> Result<Header> {
    let file = r.rest();
    if !is_bundle(file) {
        for other in ["UnityWeb", "UnityRaw", "UnityArchive"] {
            if file.starts_with(other.as_bytes()) {
                return Err(Error::Unsupported(format!("{other} bundle container")));
            }
        }
        return Err(Error::NotUnity("no UnityFS signature".into()));
    }
    r.cstr()?;
    let format = r.u32()?;
    if !(6..=8).contains(&format) {
        return Err(Error::Unsupported(format!(
            "UnityFS container format {format} (supported: 6-8)"
        )));
    }
    // A version that is not UTF-8 is not a version, as one with other odd bytes is not.
    let mut version = |what: &str| {
        r.cstr().map_err(|e| match e {
            Error::Invalid(_) => Error::NotUnity(format!(
                "{what} engine version is not a version (not UTF-8)"
            )),
            e => e,
        })
    };
    let unity_version = version("bundle player")?;
    let unity_revision = version("bundle")?;
    check_version_string(&unity_version, "bundle player")?;
    check_version_string(&unity_revision, "bundle")?;
    let declared_size = r.i64()?;
    Ok(Header {
        format,
        unity_version,
        unity_revision,
        declared_size,
    })
}

/// [`crate::file::HeadCheck`] for a bundle: its header, from its first bytes, must be one
/// this crate reads; then the file is read up to the size the header declares (anything
/// after that is not the bundle's).
pub(crate) fn check_head(head: &[u8], len: u64) -> Result<u64> {
    let mut r = Reader::new(head, true);
    let Header { declared_size, .. } = header(&mut r)?;
    declared_len(declared_size, r.pos() + REST_OF_HEADER, len)
}

/// The header's bytes after its declared size: the directory's two sizes and the flags.
const REST_OF_HEADER: usize = 12;

/// The bundle's length from its header's declared size: at least the `header` bytes already
/// read, and no more than the `len` bytes there are. Unity writes the file's own size here.
fn declared_len(declared_size: i64, header: usize, len: u64) -> Result<u64> {
    u64::try_from(declared_size)
        .ok()
        .filter(|&n| n >= header as u64 && n <= len)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "bundle header declares {declared_size} bytes; the data holds {len}"
            ))
        })
}

/// The working memory an LZMA block needs, `(tables, dictionary)` in bytes, from its
/// properties header: `lc`, `lp` and `pb` within LZMA2's bounds (Unity writes lc 3, lp 0,
/// pb 2), and a dictionary no larger than the output needs (a vector grown to it may hold
/// twice that).
fn lzma_memory(block: &[u8], size: usize) -> Result<(u64, u64)> {
    let header = block
        .get(..5)
        .ok_or_else(|| Error::Invalid("LZMA block shorter than its 5-byte header".into()))?;
    let props = u32::from(header[0]);
    let (lc, lp, pb) = (props % 9, props / 9 % 5, props / 45);
    if pb > 4 || lc + lp > 4 {
        return Err(Error::Unsupported(format!(
            "LZMA block with lc {lc}, lp {lp}, pb {pb} (lc + lp above 4 or pb above 4)"
        )));
    }
    let dictionary = u64::from(u32::from_le_bytes(
        header[1..5].try_into().unwrap_or([0; 4]),
    ));
    let tables = 2 * (0x300u64 << (lc + lp)) + 4096;
    // lzma-rs raises a dictionary under 4 KiB to 4 KiB.
    Ok((tables, 2 * dictionary.max(4096).min(size.max(4096) as u64)))
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
    out.try_reserve(size)
        .map_err(|_| Error::OutOfMemory { bytes: size as u64 })?;
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
        // The match may overlap the bytes it is producing. Its output repeats with period
        // `offset` from `start`, so each pass can copy everything written since `start`,
        // doubling the span: a one-byte run takes log2(length) copies, not length.
        let start = out.len() - offset;
        let mut left = matched;
        while left > 0 {
            let n = left.min(out.len() - start);
            out.extend_from_within(start..start + n);
            left -= n;
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
        // Offset 0 is no offset at all.
        assert!(lz4(&[0x10, b'a', 0, 0, 0x00], 5).is_err());
        // Offset 2 would reach the earlier block's bytes; this block has only one.
        assert!(lz4(&[0x10, b'a', 2, 0, 0x00], 5).is_err());
    }

    #[test]
    fn test_lz4_says_how_much_it_made() {
        // "abc", a match of 6: 9 bytes, not the 12 claimed.
        let err = lz4(&[0x32, b'a', b'b', b'c', 3, 0, 0x00], 12)
            .unwrap_err()
            .to_string();
        assert!(err.contains("gave 9 bytes, header says 12"), "{err}");
    }

    #[test]
    fn test_lz4_shortest_match_from_one_byte() {
        // "a", then the shortest match (4) at offset 1: every copy pass has one byte to copy
        // from at first.
        assert_eq!(lz4(&[0x10, b'a', 1, 0, 0x00], 5).unwrap(), b"aaaaa");
        assert!(lz4(&[0x10, b'a', 1, 0, 0x00], 4).is_err());
        assert!(lz4(&[0x10, b'a', 1, 0, 0x00], 6).is_err());
    }

    #[test]
    fn test_lz4_refuses_impossible_ratio() {
        let err = lz4(&[0x00], 1 << 20).unwrap_err().to_string();
        assert!(err.contains("more output than LZ4 can encode"), "{err}");
        // At the bound (255 to 1, plus 16) the claim is allowed through (and then fails on
        // the real length).
        let at_bound = 255 + 16;
        let err = lz4(&[0x00], at_bound).unwrap_err().to_string();
        assert!(!err.contains("more output than LZ4 can encode"), "{err}");
        let err = lz4(&[0x00], at_bound + 1).unwrap_err().to_string();
        assert!(err.contains("more output than LZ4 can encode"), "{err}");
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
