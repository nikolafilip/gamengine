//! MSB-first bit packing (PROTOCOL.md 2). The test vectors in the document are the tests here.

use crate::NetError;

/// Appends bits MSB-first. `finish` pads the last byte with zeros.
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    /// Low `partial_bits` bits are pending, right-aligned.
    partial: u8,
    partial_bits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(bytes: usize) -> Self {
        BitWriter {
            buf: Vec::with_capacity(bytes),
            partial: 0,
            partial_bits: 0,
        }
    }

    /// Bits written so far, including pending partial bits.
    pub fn bit_len(&self) -> usize {
        self.buf.len() * 8 + self.partial_bits as usize
    }

    /// Bytes the output will occupy after `finish`.
    pub fn byte_len(&self) -> usize {
        self.bit_len().div_ceil(8)
    }

    /// Write the `n` low bits of `value`, most significant first. `n <= 64`.
    pub fn write_bits(&mut self, value: u64, n: u32) {
        debug_assert!(n <= 64, "at most 64 bits per write");
        if n == 0 {
            return;
        }
        let value = if n == 64 {
            value
        } else {
            value & ((1u64 << n) - 1)
        };
        let mut remaining = n;
        while remaining > 0 {
            let free = 8 - self.partial_bits;
            let take = remaining.min(free);
            let shift = remaining - take;
            let chunk = ((value >> shift) & ((1u64 << take) - 1)) as u32;
            self.partial = (((self.partial as u32) << take) | chunk) as u8;
            self.partial_bits += take;
            if self.partial_bits == 8 {
                self.buf.push(self.partial);
                self.partial = 0;
                self.partial_bits = 0;
            }
            remaining -= take;
        }
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_bits(v as u64, 1);
    }

    /// Unsigned variable length: 7-bit groups, least significant first, each preceded by a
    /// continuation bit. Byte-aligned this is LEB128.
    pub fn write_uvar(&mut self, mut value: u64) {
        loop {
            let group = value & 0x7f;
            value >>= 7;
            let more = value != 0;
            self.write_bits(more as u64, 1);
            self.write_bits(group, 7);
            if !more {
                break;
            }
        }
    }

    /// Zigzag then `uvar`.
    pub fn write_svar(&mut self, value: i64) {
        self.write_uvar(((value << 1) ^ (value >> 63)) as u64);
    }

    /// Pad to a byte boundary and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.partial_bits > 0 {
            self.buf.push(self.partial << (8 - self.partial_bits));
        }
        self.buf
    }
}

/// Reads bits MSB-first. Every read fails with [`NetError::Overrun`] past the end.
#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// Bits left, including trailing pad bits.
    pub fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    pub fn read_bits(&mut self, n: u32) -> Result<u64, NetError> {
        debug_assert!(n <= 64, "at most 64 bits per read");
        if n == 0 {
            return Ok(0);
        }
        if self.pos + n as usize > self.data.len() * 8 {
            return Err(NetError::Overrun);
        }
        let mut out = 0u64;
        let mut remaining = n;
        while remaining > 0 {
            let byte = self.data[self.pos / 8];
            let bit_off = (self.pos % 8) as u32;
            let avail = 8 - bit_off;
            let take = remaining.min(avail);
            let shift = avail - take;
            let chunk = ((byte >> shift) as u64) & ((1u64 << take) - 1);
            out = (out << take) | chunk;
            self.pos += take as usize;
            remaining -= take;
        }
        Ok(out)
    }

    pub fn read_bool(&mut self) -> Result<bool, NetError> {
        Ok(self.read_bits(1)? != 0)
    }

    pub fn read_uvar(&mut self) -> Result<u64, NetError> {
        let mut value = 0u64;
        for i in 0..10 {
            let more = self.read_bool()?;
            let group = self.read_bits(7)?;
            if i == 9 && group > 1 {
                return Err(NetError::Malformed("uvar exceeds 64 bits"));
            }
            value |= group << (7 * i);
            if !more {
                return Ok(value);
            }
        }
        Err(NetError::Malformed("uvar has more than 10 groups"))
    }

    pub fn read_svar(&mut self) -> Result<i64, NetError> {
        let z = self.read_uvar()?;
        Ok(((z >> 1) as i64) ^ -((z & 1) as i64))
    }

    /// `read_uvar` narrowed to `u32`.
    pub fn read_uvar32(&mut self) -> Result<u32, NetError> {
        u32::try_from(self.read_uvar()?).map_err(|_| NetError::Malformed("uvar exceeds u32"))
    }

    /// `read_svar` narrowed to `i32`.
    pub fn read_svar32(&mut self) -> Result<i32, NetError> {
        i32::try_from(self.read_svar()?).map_err(|_| NetError::Malformed("svar exceeds i32"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(f: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
        let mut w = BitWriter::new();
        f(&mut w);
        w.finish()
    }

    #[test]
    fn protocol_test_vectors() {
        assert_eq!(
            bytes(|w| {
                w.write_bits(0b101, 3);
                w.write_bits(0b1111, 4);
            }),
            [0xBE]
        );
        assert_eq!(bytes(|w| w.write_uvar(0)), [0x00]);
        assert_eq!(bytes(|w| w.write_uvar(127)), [0x7F]);
        assert_eq!(bytes(|w| w.write_uvar(128)), [0x80, 0x01]);
        assert_eq!(bytes(|w| w.write_uvar(300)), [0xAC, 0x02]);
        assert_eq!(
            bytes(|w| {
                w.write_svar(0);
                w.write_svar(-1);
                w.write_svar(1);
                w.write_svar(-2);
            }),
            [0x00, 0x01, 0x02, 0x03]
        );
        assert_eq!(bytes(|w| w.write_svar(-64)), [0x7F]);
        assert_eq!(bytes(|w| w.write_svar(64)), [0x80, 0x01]);
        assert_eq!(
            bytes(|w| {
                w.write_bits(1, 1);
                w.write_uvar(300);
            }),
            [0xD6, 0x01, 0x00]
        );
        assert_eq!(
            bytes(|w| {
                w.write_bits(0xABCD, 16);
                w.write_bool(true);
            }),
            [0xAB, 0xCD, 0x80]
        );
    }

    #[test]
    fn reads_back_what_was_written() {
        let data = bytes(|w| {
            w.write_bits(5, 3);
            w.write_uvar(u64::MAX);
            w.write_svar(i64::MIN);
            w.write_svar(i64::MAX);
            w.write_bits(u64::MAX, 64);
            w.write_bool(false);
            w.write_bits(0x1234_5678_9abc, 48);
        });
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(3).unwrap(), 5);
        assert_eq!(r.read_uvar().unwrap(), u64::MAX);
        assert_eq!(r.read_svar().unwrap(), i64::MIN);
        assert_eq!(r.read_svar().unwrap(), i64::MAX);
        assert_eq!(r.read_bits(64).unwrap(), u64::MAX);
        assert!(!r.read_bool().unwrap());
        assert_eq!(r.read_bits(48).unwrap(), 0x1234_5678_9abc);
        assert!(r.remaining() < 8);
    }

    #[test]
    fn overrun_is_an_error_not_a_panic() {
        let data = [0xFFu8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0xFF);
        assert_eq!(r.read_bits(1), Err(NetError::Overrun));
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_uvar(), Err(NetError::Overrun));
        // An 11-group uvar is malformed even when the bytes exist.
        let long = [0x80u8; 11];
        let mut r = BitReader::new(&long);
        assert!(matches!(r.read_uvar(), Err(NetError::Malformed(_))));
    }

    #[test]
    fn zero_width_writes_are_noops() {
        let data = bytes(|w| {
            w.write_bits(0xFF, 0);
            w.write_bits(1, 1);
        });
        assert_eq!(data, [0x80]);
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(0).unwrap(), 0);
        assert!(r.read_bool().unwrap());
    }
}
