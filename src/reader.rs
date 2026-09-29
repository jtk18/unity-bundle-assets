//! A byte cursor with Unity's conventions: selectable endianness, 4-byte alignment after
//! strings and arrays, and length-prefixed strings.

use crate::{Error, Result};

/// Longest string read from a file: names, paths, versions. Real ones are well under 1 KiB;
/// the cap keeps every string the crate holds or quotes in an error small, whatever the file
/// says.
pub const MAX_STRING: usize = 4096;

/// A name from a file as text: invalid UTF-8 becomes U+FFFD, and the text is cut, on a
/// character boundary, to [`MAX_STRING`] bytes, so a name never costs more as text than the
/// most the file may spend on it. Held exactly.
fn text(bytes: &[u8]) -> String {
    let mut s = String::from_utf8_lossy(bytes).into_owned();
    if s.len() > MAX_STRING {
        let cut = (0..=MAX_STRING)
            .rev()
            .find(|&i| s.is_char_boundary(i))
            .unwrap_or(0);
        s.truncate(cut);
    }
    s.shrink_to_fit();
    s
}

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    big_endian: bool,
}

macro_rules! read_num {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self) -> Result<$t> {
            let bytes = self.take(std::mem::size_of::<$t>())?;
            let arr = bytes.try_into().unwrap();
            Ok(if self.big_endian {
                <$t>::from_be_bytes(arr)
            } else {
                <$t>::from_le_bytes(arr)
            })
        }
    };
}

impl<'a> Reader<'a> {
    pub const fn new(data: &'a [u8], big_endian: bool) -> Self {
        Reader {
            data,
            pos: 0,
            big_endian,
        }
    }

    /// The cursor's offset.
    pub const fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes after the cursor.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos.min(self.data.len())..]
    }

    /// Refuse an object that does not end exactly where its last field (and the alignment
    /// after it) does: bytes left over, or an alignment that stepped past the end, mean the
    /// layout was misread. `what` names the object, for the message only.
    pub fn check_end(&self, what: impl FnOnce() -> String) -> Result<()> {
        let len = self.data.len();
        if self.pos == len {
            return Ok(());
        }
        let how = if self.pos < len {
            format!("has {} bytes after its last field", len - self.pos)
        } else {
            format!(
                "ends {} bytes short of its last field's alignment",
                self.pos - len
            )
        };
        Err(Error::Invalid(format!(
            "{} {how}; the layout is probably misread",
            what()
        )))
    }

    /// Bytes left after the cursor.
    pub const fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub const fn set_big_endian(&mut self, big_endian: bool) {
        self.big_endian = big_endian;
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.data.len())
            .ok_or(Error::Truncated(self.pos))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    pub const fn align(&mut self, to: usize) {
        self.pos = self.pos.div_ceil(to) * to;
    }

    read_num!(u16, u16);
    read_num!(i16, i16);
    read_num!(u32, u32);
    read_num!(i32, i32);
    read_num!(u64, u64);
    read_num!(i64, i64);
    read_num!(f32, f32);

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    /// A NUL-terminated string's bytes, as used in serialized-file metadata, of at most
    /// [`MAX_STRING`] bytes, and the position it starts at.
    pub fn cstr_bytes(&mut self) -> Result<(usize, &'a [u8])> {
        let at = self.pos;
        let rest = &self.data[at.min(self.data.len())..];
        let len = rest
            .iter()
            .take(MAX_STRING + 1)
            .position(|&b| b == 0)
            .ok_or_else(|| {
                if rest.len() > MAX_STRING {
                    Error::Invalid(format!(
                        "string at byte {at} is longer than {MAX_STRING} bytes"
                    ))
                } else {
                    Error::Truncated(at)
                }
            })?;
        self.pos += len + 1;
        Ok((at, &rest[..len]))
    }

    /// [`Reader::cstr_bytes`] as text. Metadata strings (versions, paths) are ASCII as Unity
    /// writes them; anything that is not UTF-8 is not a file this crate reads.
    pub fn cstr(&mut self) -> Result<String> {
        let (at, bytes) = self.cstr_bytes()?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::Invalid(format!("string at byte {at} is not UTF-8")))
    }

    /// [`Reader::cstr_bytes`] as text, invalid UTF-8 shown as U+FFFD: for strings that are
    /// only reported, never used to find anything.
    pub fn cstr_lossy(&mut self) -> Result<String> {
        let (_, bytes) = self.cstr_bytes()?;
        Ok(text(bytes))
    }

    /// An array length, checked against the bytes left so a corrupt length fails cleanly
    /// instead of allocating gigabytes.
    pub fn len(&mut self, min_item_size: usize) -> Result<usize> {
        let at = self.pos;
        let n = self.i32()?;
        let left = self.data.len() - self.pos;
        if n < 0 || (n as usize).saturating_mul(min_item_size.max(1)) > left {
            return Err(Error::BadLength {
                at,
                len: i64::from(n),
            });
        }
        Ok(n as usize)
    }

    /// A length-prefixed name of at most [`MAX_STRING`] bytes, followed by alignment to 4
    /// bytes; invalid UTF-8 becomes U+FFFD.
    pub fn aligned_string(&mut self) -> Result<String> {
        let bytes = self.aligned_bytes()?;
        Ok(text(bytes))
    }

    /// A length-prefixed path, as [`Reader::aligned_string`] but refused unless UTF-8: a path
    /// names a file, and a guessed one could name the wrong file.
    pub fn aligned_path(&mut self) -> Result<String> {
        let at = self.pos;
        let bytes = self.aligned_bytes()?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| Error::Invalid(format!("path at byte {at} is not UTF-8")))
    }

    /// Step over a length-prefixed string without decoding it. Never held or shown, it is
    /// bounded only by the bytes there are, not by [`MAX_STRING`].
    pub fn skip_string(&mut self) -> Result<()> {
        let n = self.len(1)?;
        self.take(n)?;
        self.align(4);
        Ok(())
    }

    fn aligned_bytes(&mut self) -> Result<&'a [u8]> {
        let at = self.pos;
        let n = self.len(1)?;
        if n > MAX_STRING {
            return Err(Error::BadLength { at, len: n as i64 });
        }
        let bytes = self.take(n)?;
        self.align(4);
        Ok(bytes)
    }

    /// A length-prefixed byte array followed by alignment to 4 bytes.
    pub fn byte_array(&mut self) -> Result<&'a [u8]> {
        let n = self.len(1)?;
        let bytes = self.take(n)?;
        self.align(4);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_end_says_which_way_it_missed() {
        let data = [0u8; 10];
        let mut r = Reader::new(&data, false);
        r.skip(7).unwrap();
        let err = r.check_end(|| "texture".into()).unwrap_err().to_string();
        assert!(
            err.contains("texture has 3 bytes after its last field"),
            "{err}"
        );
        r.skip(3).unwrap();
        assert!(r.check_end(|| unreachable!()).is_ok());
        r.skip(0).unwrap();
        r.align(4); // 10 -> 12, past the end
        let err = r.check_end(|| "atlas".into()).unwrap_err().to_string();
        assert!(err.contains("atlas ends 2 bytes short"), "{err}");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn test_names_decode_lossily_and_metadata_strictly() {
        assert_eq!(text(b"Icon_1"), "Icon_1");
        assert_eq!(text("\u{e9}".as_bytes()), "\u{e9}");
        // Every character but the bad byte is kept.
        assert_eq!(text(b"Caf\xe9_Icon_Big"), "Caf\u{fffd}_Icon_Big");
        // 4096 bad bytes would be 12 KiB of U+FFFD: cut to the last whole character within
        // 4 KiB, and held exactly.
        let bad = text(&[0xff; 4096]);
        assert_eq!((bad.len(), bad.capacity()), (4095, 4095));
        assert!(bad.chars().all(|c| c == '\u{fffd}'));
        assert_eq!(text(&[b'a'; 4096]).len(), 4096);
        let mut r = Reader::new(b"ok\0caf\xe9\0", false);
        assert_eq!(r.cstr().unwrap(), "ok");
        assert!(matches!(r.cstr(), Err(Error::Invalid(msg)) if msg.contains("not UTF-8")));
    }
    #[test]
    fn test_strings_stop_at_4_kib() {
        let at_most = |n: usize, nul: bool| {
            let mut data = vec![b'x'; n];
            if nul {
                data.push(0);
            }
            Reader::new(&data, false).cstr().map(|s| s.len())
        };
        assert_eq!(at_most(MAX_STRING, true).unwrap(), MAX_STRING);
        assert!(matches!(
            at_most(MAX_STRING + 1, true),
            Err(Error::Invalid(_))
        ));
        // Unterminated: cut short when the data ends first, too long when it does not.
        assert!(matches!(
            at_most(MAX_STRING, false),
            Err(Error::Truncated(0))
        ));
        assert!(matches!(
            at_most(MAX_STRING + 1, false),
            Err(Error::Invalid(_))
        ));
        let prefixed = |n: usize| {
            let mut data = (n as i32).to_le_bytes().to_vec();
            data.extend(vec![b'x'; n]);
            Reader::new(&data, false).aligned_string().map(|s| s.len())
        };
        assert_eq!(prefixed(MAX_STRING).unwrap(), MAX_STRING);
        assert!(matches!(
            prefixed(MAX_STRING + 1),
            Err(Error::BadLength { at: 0, .. })
        ));
    }
}
