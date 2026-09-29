use crate::{Error, Result};
pub(crate) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| Error::decode("Offset overflow"))?;
        let out = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| Error::decode("Truncated audio"))?;
        self.pos = end;
        Ok(out)
    }
    pub fn u16(&mut self, le: bool) -> Result<u16> {
        let a: [u8; 2] = self.take(2)?.try_into().unwrap();
        Ok(if le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }
    pub fn u32(&mut self, le: bool) -> Result<u32> {
        let a: [u8; 4] = self.take(4)?.try_into().unwrap();
        Ok(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
    pub fn u64(&mut self, le: bool) -> Result<u64> {
        let a: [u8; 8] = self.take(8)?.try_into().unwrap();
        Ok(if le {
            u64::from_le_bytes(a)
        } else {
            u64::from_be_bytes(a)
        })
    }
}
pub(crate) struct Bits<'a> {
    bytes: &'a [u8],
    pub pos: usize,
}
impl<'a> Bits<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn read(&mut self, n: usize) -> Result<u32> {
        if n > 32
            || self
                .pos
                .checked_add(n)
                .is_none_or(|end| end > self.bytes.len() * 8)
        {
            return Err(Error::decode("Truncated bitstream"));
        }
        let mut out = 0;
        for _ in 0..n {
            out = out << 1 | ((self.bytes[self.pos / 8] >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Ok(out)
    }
    pub fn signed(&mut self, n: usize) -> Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        Ok((self.read(n)? as i32) << (32 - n) >> (32 - n))
    }
    pub fn peek(&self, n: usize) -> u32 {
        let mut out = 0;
        for i in 0..n {
            let p = self.pos + i;
            out = out << 1
                | self
                    .bytes
                    .get(p / 8)
                    .map_or(0, |b| ((b >> (7 - p % 8)) & 1) as u32);
        }
        out
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.read(n).map(|_| ())
    }
    pub fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }
}
pub(crate) struct BitWriter {
    pub bytes: Vec<u8>,
    pub pos: usize,
}
impl BitWriter {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            pos: 0,
        }
    }
    pub fn write(&mut self, value: u64, n: usize) {
        for i in (0..n).rev() {
            if self.pos % 8 == 0 {
                self.bytes.push(0);
            }
            self.bytes[self.pos / 8] |= (((value >> i) & 1) as u8) << (7 - self.pos % 8);
            self.pos += 1;
        }
    }
    pub fn align(&mut self) {
        let n = (8 - self.pos % 8) % 8;
        self.write(0, n);
    }
}
pub(crate) fn put16(out: &mut Vec<u8>, n: u16, le: bool) {
    out.extend_from_slice(&if le { n.to_le_bytes() } else { n.to_be_bytes() });
}
pub(crate) fn put32(out: &mut Vec<u8>, n: u32, le: bool) {
    out.extend_from_slice(&if le { n.to_le_bytes() } else { n.to_be_bytes() });
}
pub(crate) fn put64(out: &mut Vec<u8>, n: u64, le: bool) {
    out.extend_from_slice(&if le { n.to_le_bytes() } else { n.to_be_bytes() });
}
