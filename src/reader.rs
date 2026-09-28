//! A byte cursor with Unity's conventions: selectable endianness, 4-byte alignment after
//! strings and arrays, and length-prefixed strings.

use crate::{Error, Result};

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
    pub fn new(data: &'a [u8], big_endian: bool) -> Self {
        Reader {
            data,
            pos: 0,
            big_endian,
        }
    }

    pub fn set_big_endian(&mut self, big_endian: bool) {
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

    pub fn align(&mut self, to: usize) {
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

    /// A NUL-terminated string, as used in serialized-file metadata.
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.data[self.pos.min(self.data.len())..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::Truncated(self.pos))?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }

    /// An array length, checked against the bytes left so a corrupt length fails cleanly
    /// instead of allocating gigabytes.
    pub fn len(&mut self, min_item_size: usize) -> Result<usize> {
        let at = self.pos;
        let n = self.i32()?;
        let left = self.data.len() - self.pos;
        if n < 0 || (n as usize).saturating_mul(min_item_size.max(1)) > left {
            return Err(Error::BadLength { at, len: n as i64 });
        }
        Ok(n as usize)
    }

    /// A length-prefixed string followed by alignment to 4 bytes.
    pub fn aligned_string(&mut self) -> Result<String> {
        let n = self.len(1)?;
        let s = String::from_utf8_lossy(self.take(n)?).into_owned();
        self.align(4);
        Ok(s)
    }

    /// A length-prefixed byte array followed by alignment to 4 bytes.
    pub fn byte_array(&mut self) -> Result<&'a [u8]> {
        let n = self.len(1)?;
        let bytes = self.take(n)?;
        self.align(4);
        Ok(bytes)
    }
}
