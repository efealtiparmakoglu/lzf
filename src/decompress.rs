//! Decompression: block framing parser, Huffman table reconstruction and the
//! LZ77 copy loop. Every malformed input surfaces as `Error::Corrupt`, never
//! as a panic.

use std::io::{self, Read, Write};

use crate::bitio::BitReader;
use crate::crc32::Crc32;
use crate::error::{Error, Result};
use crate::format::{self, Header, BLOCK_SIZE};
use crate::huffman::{unrle_code_lengths, DecodeTable};
use crate::tables::{
    symbol_to_dist, symbol_to_dist_extra, symbol_to_length, symbol_to_length_extra, DIST_ALPHABET,
    LITLEN_ALPHABET, WINDOW,
};

/// A `Write` adapter that CRCs and counts everything passing through.
pub struct CountingCrcWriter<W: Write> {
    inner: W,
    crc: Crc32,
    pub bytes: u64,
}

impl<W: Write> CountingCrcWriter<W> {
    pub fn new(inner: W) -> Self {
        CountingCrcWriter { inner, crc: Crc32::new(), bytes: 0 }
    }
    pub fn crc(&self) -> u32 {
        self.crc.finalize()
    }
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingCrcWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.crc.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Streaming decompressor: reads the header, all blocks and the trailer from
/// `r`, writing verified output to `w`. Returns the header, the number of
/// output bytes and the writer back.
pub fn decompress_stream<R: Read, W: Write>(r: R, w: W) -> Result<(Header, u64, W)> {
    let mut r = r;
    let header = format::read_header(&mut r)?;
    let mut out = CountingCrcWriter::new(w);
    let mut block_buf: Vec<u8> = Vec::new();

    loop {
        let mut bh = [0u8; 1];
        read_exact(&mut r, &mut bh)?;
        let is_final = bh[0] & 1 != 0;
        let block_type = bh[0] >> 1;

        match block_type {
            0 => {
                let len = read_u32(&mut r)? as usize;
                if len > BLOCK_SIZE {
                    return Err(Error::Corrupt("raw block exceeds block size"));
                }
                let copied = io::copy(&mut (&mut r).take(len as u64), &mut out)?;
                if copied != len as u64 {
                    return Err(Error::Corrupt("truncated raw block"));
                }
            }
            1 => {
                let raw_len = read_u32(&mut r)? as usize;
                let payload_len = read_u32(&mut r)? as usize;
                if raw_len > BLOCK_SIZE {
                    return Err(Error::Corrupt("block exceeds block size"));
                }
                // A payload this size can never be legitimate; refuse early
                // instead of allocating whatever the stream asks for.
                if payload_len > BLOCK_SIZE + (1 << 16) {
                    return Err(Error::Corrupt("implausible payload length"));
                }
                let mut payload = vec![0u8; payload_len];
                read_exact(&mut r, &mut payload)?;
                decode_lz_block(&payload, raw_len, &mut block_buf)?;
                out.write_all(&block_buf)?;
            }
            _ => return Err(Error::Corrupt("unknown block type")),
        }
        if is_final {
            break;
        }
    }

    let mut trailer = [0u8; 12];
    read_exact(&mut r, &mut trailer)?;
    let orig_size = u64::from_le_bytes(trailer[..8].try_into().unwrap());
    let stored_crc = u32::from_le_bytes(trailer[8..].try_into().unwrap());
    let actual_crc = out.crc();
    if out.bytes != orig_size {
        return Err(Error::Corrupt("size mismatch"));
    }
    if actual_crc != stored_crc {
        return Err(Error::ChecksumMismatch { expected: stored_crc, actual: actual_crc });
    }
    // Nothing but EOF may follow the trailer.
    let mut extra = [0u8; 1];
    if r.read(&mut extra)? != 0 {
        return Err(Error::Corrupt("trailing garbage after trailer"));
    }
    let written = out.bytes;
    let w = out.into_inner();
    Ok((header, written, w))
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    r.read_exact(buf).map_err(|_| Error::Corrupt("unexpected end of file"))
}

fn read_u32<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    read_exact(r, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// Decodes one LZ block payload into `out` (cleared first).
fn decode_lz_block(payload: &[u8], raw_len: usize, out: &mut Vec<u8>) -> Result<()> {
    if raw_len == 0 {
        return Err(Error::Corrupt("empty compressed block"));
    }
    let mut br = BitReader::new(payload);

    // Meta table: 19 nibbles.
    let mut meta_lens = [0u8; 19];
    for slot in meta_lens.iter_mut() {
        *slot = br.read_bits(4)? as u8;
    }
    let meta = DecodeTable::new(&meta_lens)?;

    let hlit = br.read_bits(9)? as usize + 1;
    let hdist = br.read_bits(6)? as usize + 1;
    if hlit > LITLEN_ALPHABET || hdist > DIST_ALPHABET {
        return Err(Error::Corrupt("alphabet size out of range"));
    }
    let lens = unrle_code_lengths(hlit + hdist, &meta, &mut br)?;
    let (lit_lens, dist_lens) = lens.split_at(hlit);
    if !lit_lens.iter().any(|&l| l > 0) {
        return Err(Error::Corrupt("literal/length table has no codes"));
    }
    if !dist_lens.iter().any(|&l| l > 0) {
        return Err(Error::Corrupt("distance table has no codes"));
    }
    let lit = DecodeTable::new(lit_lens)?;
    let dist = DecodeTable::new(dist_lens)?;

    out.clear();
    out.reserve(raw_len);
    while out.len() < raw_len {
        let sym = lit.decode(&mut br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256..=287 => {
                let (base, _extra) = symbol_to_length(sym);
                let len = base + br.read_bits(symbol_to_length_extra(sym))? as usize;
                if out.len() + len > raw_len {
                    return Err(Error::Corrupt("match overruns block end"));
                }
                let dsym = dist.decode(&mut br)?;
                if dsym as usize >= DIST_ALPHABET {
                    return Err(Error::Corrupt("invalid distance symbol"));
                }
                let (dbase, _) = symbol_to_dist(dsym);
                let d = dbase + br.read_bits(symbol_to_dist_extra(dsym))? as usize;
                if d > out.len() {
                    return Err(Error::Corrupt("distance reaches before block start"));
                }
                if d > WINDOW {
                    return Err(Error::Corrupt("distance exceeds window"));
                }
                if d >= len {
                    let start = out.len() - d;
                    out.extend_from_within(start..start + len);
                } else {
                    // Overlapping copy: byte-by-byte so the source trails the
                    // destination (this is how runs get expanded).
                    let start = out.len() - d;
                    for i in 0..len {
                        let b = out[start + i];
                        out.push(b);
                    }
                }
            }
            _ => return Err(Error::Corrupt("invalid literal/length symbol")),
        }
    }
    Ok(())
}

/// In-memory helper used by tests and `decompress_bytes`.
pub fn decompress_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let (_, _, cursor) =
        decompress_stream(io::Cursor::new(data), io::Cursor::new(Vec::new()))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::compress_bytes;

    #[test]
    fn rejects_bad_magic() {
        let mut data = compress_bytes(b"hello", 6, b"x").unwrap();
        data[0] = b'X';
        assert!(matches!(
            decompress_bytes(&data),
            Err(Error::Corrupt("bad magic"))
        ));
    }

    #[test]
    fn rejects_crc_corruption() {
        let data = compress_bytes(b"some reasonably repetitive payload payload payload", 6, b"x")
            .unwrap();
        let mut data = data;
        let last = data.len() - 1; // flip a CRC trailer byte
        data[last] ^= 0xFF;
        assert!(decompress_bytes(&data).is_err());
    }

    #[test]
    fn rejects_payload_bitflip() {
        let data = compress_bytes(
            &[7u8; 10_000], // heavily compressed; payload exists
            6,
            b"x",
        )
        .unwrap();
        // Flip a byte somewhere inside the first compressed payload.
        let mut data = data;
        data[20] ^= 0x01;
        // Corruption must be detected (CRC), never a panic.
        assert!(decompress_bytes(&data).is_err());
    }

    #[test]
    fn rejects_truncation() {
        let data = compress_bytes(&vec![3u8; 50_000], 6, b"x").unwrap();
        for cut in [0, 5, 12, 30, data.len() / 2] {
            assert!(decompress_bytes(&data[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn rejects_trailing_garbage() {
        let mut data = compress_bytes(b"payload", 6, b"x").unwrap();
        data.push(0);
        assert!(matches!(
            decompress_bytes(&data),
            Err(Error::Corrupt("trailing garbage after trailer"))
        ));
    }

    #[test]
    fn zero_length_compressed_block_rejected() {
        // Hand-craft: header + a final LZ block claiming raw_len = 0.
        let mut w = Vec::new();
        format::write_header(&mut w, b"x").unwrap();
        let mut block = vec![(true as u8) | (1 << 1)];
        block.extend_from_slice(&0u32.to_le_bytes()); // raw_len = 0
        block.extend_from_slice(&1u32.to_le_bytes()); // payload_len = 1
        block.push(0);
        block.extend_from_slice(&0u64.to_le_bytes()); // size
        block.extend_from_slice(&crate::crc32::checksum(b"").to_le_bytes());
        w.extend_from_slice(&block);
        assert!(decompress_bytes(&w).is_err());
    }

    #[test]
    fn stream_reports_verified_size() {
        use crate::compress::compress_stream;
        let data = vec![9u8; 1000];
        let (input, _, _) =
            compress_stream(io::Cursor::new(&data), io::Cursor::new(Vec::new()), 6, b"x")
                .unwrap();
        assert_eq!(input, 1000);
        let (_, written, _) =
            decompress_stream(io::Cursor::new(&compress_bytes(&data, 6, b"x").unwrap()), io::Cursor::new(Vec::new()))
                .unwrap();
        assert_eq!(written, 1000);
    }
}
