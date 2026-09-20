//! MSB-first bit I/O.
//!
//! The LZF1 bitstream packs bits starting from the most significant bit of
//! each byte. `BitWriter` maintains a small accumulator and writes whole
//! bytes; `BitReader` reads from an in-memory slice with a 64-bit window.

use crate::error::{Error, Result};
use std::io::{self, Write};

const W_BUF: usize = 8192;

/// Packs bits MSB-first into an underlying writer.
pub struct BitWriter<W: Write> {
    out: W,
    buf: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl<W: Write> BitWriter<W> {
    pub fn new(out: W) -> Self {
        BitWriter {
            out,
            buf: Vec::with_capacity(W_BUF + 8),
            acc: 0,
            nbits: 0,
        }
    }

    /// Appends the low `n` bits of `value` (n <= 32).
    pub fn write_bits(&mut self, value: u32, n: u32) -> io::Result<()> {
        debug_assert!(n <= 32);
        let v = if n >= 32 {
            value as u64
        } else {
            (value as u64) & ((1u64 << n) - 1)
        };
        // Invariant: nbits <= 7 between calls, so acc never exceeds 39 bits.
        self.acc = (self.acc << n) | v;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.buf.push((self.acc >> self.nbits) as u8);
        }
        if self.buf.len() >= W_BUF {
            self.out.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    /// Pads the current byte with zero bits. Call after the last `write_bits`
    /// of a bitstream.
    pub fn byte_align(&mut self) -> io::Result<()> {
        if self.nbits > 0 {
            let byte = ((self.acc << (8 - self.nbits)) & 0xFF) as u8;
            self.buf.push(byte);
            self.nbits = 0;
        }
        self.acc = 0;
        Ok(())
    }

    /// Flushes the byte buffer into the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        if !self.buf.is_empty() {
            let buf = std::mem::take(&mut self.buf);
            self.out.write_all(&buf)?;
        }
        self.out.flush()
    }

    /// Consumes the writer and returns the underlying writer.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(self.out)
    }
}

/// Reads bits MSB-first from a byte slice.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    #[inline]
    pub fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// The next <=64 bits, next bit at the top; missing bytes read as zero.
    #[inline]
    fn peek_u64(&self) -> u64 {
        let byte = self.pos >> 3;
        if byte >= self.data.len() {
            return 0;
        }
        let end = (byte + 8).min(self.data.len());
        let mut buf = [0u8; 8];
        buf[..end - byte].copy_from_slice(&self.data[byte..end]);
        u64::from_be_bytes(buf) << (self.pos & 7)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Result<u32> {
        if self.bits_left() == 0 {
            return Err(Error::Corrupt("unexpected end of bitstream"));
        }
        let bit = (self.peek_u64() >> 63) as u32;
        self.pos += 1;
        Ok(bit)
    }

    #[inline]
    pub fn read_bits(&mut self, n: u32) -> Result<u32> {
        debug_assert!(n <= 32);
        if n == 0 {
            return Ok(0);
        }
        if n as usize > self.bits_left() {
            return Err(Error::Corrupt("unexpected end of bitstream"));
        }
        let v = (self.peek_u64() >> (64 - n)) as u32;
        self.pos += n as usize;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_roundtrip() {
        let mut w = BitWriter::new(Vec::new());
        w.write_bits(0b101, 3).unwrap();
        w.write_bits(0xFFFF, 16).unwrap();
        w.write_bits(0b0001111, 4).unwrap();
        w.byte_align().unwrap();
        let bytes = w.into_inner().unwrap();

        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(16).unwrap(), 0xFFFF);
        assert_eq!(r.read_bits(4).unwrap(), 0b0001111);
        assert!(r.bits_left() < 8); // only padding remains
    }

    #[test]
    fn byte_align_pads() {
        let mut w = BitWriter::new(Vec::new());
        w.write_bits(0b1, 1).unwrap();
        w.byte_align().unwrap();
        let bytes = w.into_inner().unwrap();
        assert_eq!(bytes, vec![0b1000_0000]);
    }

    #[test]
    fn reading_past_end_is_error() {
        let mut w = BitWriter::new(Vec::new());
        w.write_bits(1, 1).unwrap();
        w.byte_align().unwrap();
        let bytes = w.into_inner().unwrap();
        let mut r = BitReader::new(&bytes);
        r.read_bits(8).unwrap();
        assert!(r.read_bit().is_err());
    }
}
