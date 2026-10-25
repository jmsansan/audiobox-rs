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
        if n == 0 {
            return Ok(0);
        }
        let out = self.window(n);
        self.pos += n;
        Ok(out)
    }
    /// The `n` (1..=32) bits from the current position, most significant first, with
    /// zeros past the end: eight bytes at once rather than a bit at a time.
    fn window(&self, n: usize) -> u32 {
        let (byte, shift) = (self.pos / 8, self.pos % 8);
        let word = match self.bytes.get(byte..byte + 8) {
            Some(b) => u64::from_be_bytes(b.try_into().unwrap()),
            None => (0..8).fold(0u64, |w, i| {
                w << 8 | self.bytes.get(byte + i).copied().unwrap_or(0) as u64
            }),
        };
        // `shift + n` is at most 39, inside the 64 loaded bits.
        ((word << shift) >> (64 - n)) as u32
    }
    pub fn signed(&mut self, n: usize) -> Result<i32> {
        if n == 0 {
            return Ok(0);
        }
        Ok((self.read(n)? as i32) << (32 - n) >> (32 - n))
    }
    pub fn peek(&self, n: usize) -> u32 {
        if (1..=32).contains(&n) && self.pos <= usize::MAX - 64 {
            return self.window(n);
        }
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
    /// Bits left to read.
    pub fn remaining(&self) -> usize {
        (self.bytes.len() * 8).saturating_sub(self.pos)
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

#[cfg(test)]
mod tests {
    use super::Bits;

    /// The bit-at-a-time reader of 0.4.0.
    fn reference_read(bytes: &[u8], pos: &mut usize, n: usize) -> Option<u32> {
        if n > 32 || pos.checked_add(n).is_none_or(|end| end > bytes.len() * 8) {
            return None;
        }
        let mut out = 0;
        for _ in 0..n {
            out = out << 1 | ((bytes[*pos / 8] >> (7 - *pos % 8)) & 1) as u32;
            *pos += 1;
        }
        Some(out)
    }
    fn reference_peek(bytes: &[u8], pos: usize, n: usize) -> u32 {
        (0..n).fold(0, |out, i| {
            let p = pos + i;
            out << 1
                | bytes
                    .get(p / 8)
                    .map_or(0, |b| ((b >> (7 - p % 8)) & 1) as u32)
        })
    }

    #[test]
    fn whole_byte_reads_match_bit_by_bit_reads() {
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for len in [0, 1, 2, 5, 8, 9, 13, 64] {
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            for _ in 0..200 {
                let mut br = Bits::new(&bytes);
                let mut pos = 0;
                loop {
                    let n = next() as usize % 34;
                    for k in [0, 1, 7, 19, 32, 40] {
                        assert_eq!(
                            br.peek(k),
                            reference_peek(&bytes, pos, k),
                            "peek {k} at {pos}"
                        );
                    }
                    match (br.read(n), reference_read(&bytes, &mut pos, n)) {
                        (Ok(a), Some(b)) => assert_eq!(a, b, "read {n}"),
                        (Err(_), None) => break,
                        (a, b) => panic!("read {n} at {}: {a:?} vs {b:?}", br.pos),
                    }
                    assert_eq!(br.pos, pos);
                    assert_eq!(br.remaining(), len * 8 - pos);
                }
            }
        }
    }
}
