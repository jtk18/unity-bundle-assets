//! Unity asset bundles (`UnityFS`): a header, a table of compressed blocks, and a directory of
//! the files stored in the decompressed block stream: usually one serialized file (`CAB-...`)
//! and the `.resS` / `.resource` stream files its textures and audio point into.
//!
//! The layout follows UnityPy's `BundleFile` reader (MIT, see NOTICE).

use crate::reader::Reader;
use crate::{Error, Result};

/// The signature a bundle starts with, NUL included.
pub const SIGNATURE: &[u8] = b"UnityFS\0";

/// Refuse bundles claiming more than this much decompressed data.
const MAX_UNCOMPRESSED: u64 = 4 << 30;

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

/// One file inside a bundle.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Entry {
    /// The file's name in the bundle's directory, e.g. `CAB-<hash>` or `CAB-<hash>.resS`.
    pub path: String,
    /// Directory flags; bit 2 marks a serialized file.
    pub flags: u32,
    offset: usize,
    size: usize,
}

impl Entry {
    /// Whether this entry is a serialized file rather than a stream file.
    pub fn is_serialized(&self) -> bool {
        self.flags & flags::NODE_SERIALIZED != 0
    }
}

/// A parsed bundle. Holds the whole decompressed block stream.
#[non_exhaustive]
pub struct Bundle {
    /// Container format version (6 for Unity 5.x-2019.3, 7 from 2019.4, 8 from 2022).
    pub format: u32,
    /// The player version string, e.g. `5.x.x`.
    pub unity_version: String,
    /// The engine release that built the bundle, e.g. `2018.4.36f1`.
    pub unity_revision: String,
    /// Every file in the bundle, in directory order.
    pub entries: Vec<Entry>,
    data: Vec<u8>,
}

/// Whether `data` starts like an asset bundle.
pub fn is_bundle(data: &[u8]) -> bool {
    data.starts_with(SIGNATURE)
}

impl Bundle {
    /// Read and parse the bundle at `path`.
    pub fn open(path: &std::path::Path) -> Result<Bundle> {
        Bundle::parse(&std::fs::read(path)?)
    }

    /// Parse a bundle from its bytes, decompressing every block.
    pub fn parse(file: &[u8]) -> Result<Bundle> {
        let mut r = Reader::new(file, true);
        let signature = r.cstr()?;
        if signature != "UnityFS" {
            return Err(Error::Unsupported(format!(
                "bundle signature {signature:?} (only UnityFS is read)"
            )));
        }
        let format = r.u32()?;
        let unity_version = r.cstr()?;
        let unity_revision = r.cstr()?;
        let _size = r.i64()?;
        let info_compressed = r.u32()? as usize;
        let info_size = r.u32()? as usize;
        let flags = r.u32()?;

        let revision = version_numbers(&unity_revision);
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
            return Err(Error::Unsupported("encrypted asset bundle".into()));
        }
        if format >= 7 || (revision[0] == 2019 && revision >= [2019, 4, 15]) {
            r.align(16);
        }

        let info_bytes = if flags & flags::BLOCKS_INFO_AT_END != 0 {
            let start = file
                .len()
                .checked_sub(info_compressed)
                .ok_or(Error::Truncated(file.len()))?;
            &file[start..]
        } else {
            r.take(info_compressed)?
        };
        let info = decompress(info_bytes, info_size, flags & flags::COMPRESSION_MASK)?;
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
        if total > MAX_UNCOMPRESSED {
            return Err(Error::Invalid(format!(
                "bundle claims {total} bytes uncompressed"
            )));
        }
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
        let mut data = Vec::with_capacity(total as usize);
        for (size, compressed, block_flags) in blocks {
            let block = r.take(compressed)?;
            data.extend(decompress(
                block,
                size,
                block_flags & flags::COMPRESSION_MASK,
            )?);
        }

        Ok(Bundle {
            format,
            unity_version,
            unity_revision,
            entries,
            data,
        })
    }

    /// One entry's bytes.
    pub fn bytes(&self, entry: &Entry) -> &[u8] {
        &self.data[entry.offset..entry.offset + entry.size]
    }

    /// The entry with this path, or with this file name (stream paths look like
    /// `archive:/CAB-.../CAB-....resS`; the directory lists `CAB-....resS`).
    pub fn entry(&self, path: &str) -> Option<&Entry> {
        let name = path.rsplit('/').next().unwrap_or(path);
        self.entries
            .iter()
            .find(|e| e.path == path)
            .or_else(|| self.entries.iter().find(|e| e.path == name))
    }

    /// The entries holding serialized files.
    pub fn serialized_files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_serialized())
    }
}

fn version_numbers(version: &str) -> [u32; 3] {
    let mut out = [0; 3];
    for (slot, part) in out.iter_mut().zip(
        version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty()),
    ) {
        *slot = part.parse().unwrap_or(0);
    }
    out
}

fn decompress(data: &[u8], size: usize, compression: u32) -> Result<Vec<u8>> {
    match compression {
        0 => {
            if data.len() != size {
                return Err(Error::Invalid(format!(
                    "stored block is {} bytes, header says {size}",
                    data.len()
                )));
            }
            Ok(data.to_vec())
        }
        1 => lzma(data, size),
        2 | 3 => lz4(data, size),
        other => Err(Error::Unsupported(format!(
            "bundle compression {other} (supported: none, LZMA, LZ4, LZ4HC)"
        ))),
    }
}

/// Unity's LZMA: the 5-byte properties header, then the raw stream, with no size field.
fn lzma(data: &[u8], size: usize) -> Result<Vec<u8>> {
    use lzma_rs::decompress::{Options, UnpackedSize};
    let options = Options {
        unpacked_size: UnpackedSize::UseProvided(Some(size as u64)),
        // The dictionary never needs to be larger than what it produces.
        memlimit: Some(size.max(4096)),
        allow_incomplete: false,
    };
    let mut out = Vec::with_capacity(size);
    lzma_rs::lzma_decompress_with_options(&mut &data[..], &mut out, &options)
        .map_err(|e| Error::Invalid(format!("LZMA block: {e}")))?;
    if out.len() != size {
        return Err(Error::Invalid(format!(
            "LZMA block gave {} bytes, header says {size}",
            out.len()
        )));
    }
    Ok(out)
}

/// An LZ4 block (not frame): sequences of literals and back-references. LZ4HC produces the
/// same format.
fn lz4(data: &[u8], size: usize) -> Result<Vec<u8>> {
    let bad = |what: &str| Error::Invalid(format!("LZ4 block: {what}"));
    let mut out: Vec<u8> = Vec::with_capacity(size);
    let mut i = 0;
    let length = |i: &mut usize, mut n: usize| -> Result<usize> {
        if n == 15 {
            loop {
                let b = *data.get(*i).ok_or_else(|| bad("truncated length"))?;
                *i += 1;
                n += b as usize;
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
        let src = data
            .get(i..i + literals)
            .ok_or_else(|| bad("literals run past the input"))?;
        if out.len() + literals > size {
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
        if offset == 0 || offset > out.len() {
            return Err(bad("offset outside the output"));
        }
        if out.len() + matched > size {
            return Err(bad("output larger than the header says"));
        }
        let start = out.len() - offset;
        // Byte by byte: a match may overlap the bytes it is producing.
        for k in 0..matched {
            let b = out[start + k];
            out.push(b);
        }
    }
    if out.len() != size {
        return Err(bad(&format!(
            "gave {} bytes, header says {size}",
            out.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_lz4_long_lengths() {
        // 20 literals: token nibble 15, then 5.
        let mut block = vec![0xf0, 5];
        block.extend(0..20u8);
        assert_eq!(lz4(&block, 20).unwrap(), (0..20u8).collect::<Vec<_>>());
    }

    #[test]
    fn test_not_a_bundle() {
        assert!(matches!(
            Bundle::parse(b"UnityWeb\0\0\0\0\x06"),
            Err(Error::Unsupported(_))
        ));
    }
}
