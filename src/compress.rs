//! Compression: tokenization (LZ77 with lazy matching), per-block Huffman
//! table construction, bitstream encoding and block framing.

use std::io::{self, Read, Write};

use crate::bitio::BitWriter;
use crate::crc32::Crc32;
use crate::error::Result;
use crate::format::{self, BLOCK_SIZE};
use crate::huffman::{build_lengths, canonical_codes, rle_code_lengths, MAX_BITS, META_MAX_BITS};
use crate::matchfind::{params, MatchFinder};
use crate::tables::{
    dist_to_symbol, length_to_symbol, symbol_to_dist_extra, symbol_to_length_extra,
    DIST_ALPHABET, LITLEN_ALPHABET, META_ALPHABET,
};

#[derive(Clone, Copy, Debug)]
pub enum Token {
    Lit(u8),
    Match { len: u16, dist: u32 },
}

/// Result of a compression run.
#[derive(Clone, Copy, Debug)]
pub struct Stats {
    pub input_size: u64,
    pub output_size: u64,
}

/// Streaming compressor: write 1 MiB chunks via [`compress_chunk`], then
/// [`finish`]. Each chunk becomes one independent LZF1 block (own Huffman
/// tables, own window).
pub struct Compressor<W: Write> {
    out: W,
    level: u8,
    crc: Crc32,
    total_in: u64,
    out_count: u64,
    finder: MatchFinder,
    tokens: Vec<Token>,
    payload: Vec<u8>,
    lit_freq: Vec<u32>,
    dist_freq: Vec<u32>,
}

impl<W: Write> Compressor<W> {
    /// Writes the LZF1 header (magic, version, stored name) and prepares the
    /// token buffers. `name` is informational metadata only.
    pub fn new(mut out: W, level: u8, name: &[u8]) -> Result<Self> {
        let n = format::write_header(&mut out, name)?;
        Ok(Compressor {
            out_count: n as u64,
            out,
            level,
            crc: Crc32::new(),
            total_in: 0,
            finder: MatchFinder::new(BLOCK_SIZE),
            tokens: Vec::with_capacity(BLOCK_SIZE),
            payload: Vec::with_capacity(BLOCK_SIZE + 4096),
            lit_freq: vec![0; LITLEN_ALPHABET],
            dist_freq: vec![0; DIST_ALPHABET],
        })
    }

    /// Emits one block for `chunk`. The last call must pass `is_final = true`
    /// (an empty final chunk produces a zero-length raw block).
    pub fn compress_chunk(&mut self, chunk: &[u8], is_final: bool) -> Result<()> {
        self.crc.update(chunk);
        self.total_in += chunk.len() as u64;

        if self.level == 0 {
            self.write_raw_block(chunk, is_final)?;
            return Ok(());
        }

        self.tokens.clear();
        self.tokenize(chunk);
        if self.encode_block(chunk, is_final)? {
            return Ok(());
        }
        // Compression did not pay off: fall back to a raw block.
        self.write_raw_block(chunk, is_final)
    }

    /// Writes the trailer (original size + CRC-32) and flushes.
    pub fn finish(mut self) -> Result<(W, Stats)> {
        let mut trailer = [0u8; 12];
        trailer[..8].copy_from_slice(&self.total_in.to_le_bytes());
        trailer[8..].copy_from_slice(&self.crc.finalize().to_le_bytes());
        self.out.write_all(&trailer)?;
        self.out.flush()?;
        self.out_count += trailer.len() as u64;
        Ok((
            self.out,
            Stats { input_size: self.total_in, output_size: self.out_count },
        ))
    }

    /// LZ77 tokenization with gzip-style lazy matching: a match found at
    /// position i is held one step while position i+1 is probed; the longer
    /// match wins and the loser decays into a literal.
    fn tokenize(&mut self, data: &[u8]) {
        let p = params(self.level);
        self.finder.reset();
        let mut i = 0usize;
        // A match found at position i-1, held during the one-step lookahead.
        let mut pending: Option<(usize, u32)> = None;

        while i < data.len() {
            let cur = self.finder.insert_and_find(data, i, &p);
            match pending {
                None => match cur {
                    // No decision pending: hold this match one step, or emit
                    // the plain literal.
                    Some(_) => {
                        pending = cur;
                        i += 1;
                    }
                    None => {
                        self.tokens.push(Token::Lit(data[i]));
                        i += 1;
                    }
                },
                Some((plen, pdist)) => match cur {
                    Some((len, _)) if len > plen => {
                        // The step-lookahead found better: the held match at
                        // i-1 decays into a literal, the new match is held.
                        self.tokens.push(Token::Lit(data[i - 1]));
                        pending = cur;
                        i += 1;
                    }
                    _ => {
                        // Emit the held match at i-1 and skip over it,
                        // registering every covered position in the chains.
                        self.tokens.push(Token::Match { len: plen as u16, dist: pdist });
                        let end = i - 1 + plen;
                        for pos in i..end {
                            self.finder.insert(data, pos);
                        }
                        pending = None;
                        i = end;
                    }
                },
            }
        }
        if let Some((plen, pdist)) = pending {
            self.tokens.push(Token::Match { len: plen as u16, dist: pdist });
        }
    }

    /// Counts frequencies, builds the three Huffman tables and serializes the
    /// block. Returns `Ok(false)` when the payload would not beat storing.
    fn encode_block(&mut self, chunk: &[u8], is_final: bool) -> Result<bool> {
        // A nonempty chunk always tokenizes to at least one token, but fall
        // back to raw rather than emit a table-less block.
        if self.tokens.is_empty() {
            return Ok(false);
        }
        for f in self.lit_freq.iter_mut() {
            *f = 0;
        }
        for f in self.dist_freq.iter_mut() {
            *f = 0;
        }
        for tok in &self.tokens {
            match *tok {
                Token::Lit(b) => self.lit_freq[b as usize] += 1,
                Token::Match { len, dist } => {
                    let (lsym, _) = length_to_symbol(len as usize);
                    self.lit_freq[lsym as usize] += 1;
                    let (dsym, _) = dist_to_symbol(dist as usize);
                    self.dist_freq[dsym as usize] += 1;
                }
            }
        }
        // The distance alphabet must always transmit at least one code even
        // when the block contains no matches.
        if self.dist_freq.iter().all(|&f| f == 0) {
            self.dist_freq[0] = 1;
        }

        let lit_lens = build_lengths(&self.lit_freq, MAX_BITS);
        let dist_lens = build_lengths(&self.dist_freq, MAX_BITS);
        let hlit = lit_lens
            .iter()
            .rposition(|&l| l > 0)
            .map(|p| p + 1)
            .unwrap_or(1);
        let hdist = dist_lens
            .iter()
            .rposition(|&l| l > 0)
            .map(|p| p + 1)
            .unwrap_or(1);

        // One code-length stream for both alphabets, RLE'd, then Huffman
        // coded with its own (7-bit-limited) table.
        let mut combined: Vec<u8> = Vec::with_capacity(hlit + hdist);
        combined.extend_from_slice(&lit_lens[..hlit]);
        combined.extend_from_slice(&dist_lens[..hdist]);
        let packed = rle_code_lengths(&combined);

        let mut meta_freq = [0u32; META_ALPHABET];
        for (sym, _) in &packed {
            meta_freq[*sym as usize] += 1;
        }
        let meta_lens = build_lengths(&meta_freq, META_MAX_BITS);
        let meta_codes = canonical_codes(&meta_lens);

        let lit_codes = canonical_codes(&lit_lens);
        let dist_codes = canonical_codes(&dist_lens);

        let mut bw = BitWriter::new(&mut self.payload);
        for &l in &meta_lens {
            bw.write_bits(l as u32, 4)?;
        }
        bw.write_bits((hlit - 1) as u32, 9)?;
        bw.write_bits((hdist - 1) as u32, 6)?;
        for (sym, extra) in &packed {
            let l = meta_lens[*sym as usize] as u32;
            bw.write_bits(meta_codes[*sym as usize] as u32, l)?;
            let extra_bits = match *sym {
                16 => 2,
                17 => 3,
                18 => 7,
                _ => 0,
            };
            if extra_bits > 0 {
                bw.write_bits(*extra, extra_bits)?;
            }
        }
        for tok in &self.tokens {
            match *tok {
                Token::Lit(b) => {
                    let sym = b as usize;
                    bw.write_bits(lit_codes[sym] as u32, lit_lens[sym] as u32)?;
                }
                Token::Match { len, dist } => {
                    let (lsym, lextra) = length_to_symbol(len as usize);
                    let li = lsym as usize;
                    bw.write_bits(lit_codes[li] as u32, lit_lens[li] as u32)?;
                    let e = symbol_to_length_extra(lsym);
                    if e > 0 {
                        bw.write_bits(lextra, e)?;
                    }
                    let (dsym, dextra) = dist_to_symbol(dist as usize);
                    let di = dsym as usize;
                    bw.write_bits(dist_codes[di] as u32, dist_lens[di] as u32)?;
                    let e = symbol_to_dist_extra(dsym);
                    if e > 0 {
                        bw.write_bits(dextra, e)?;
                    }
                }
            }
        }
        bw.byte_align()?;
        bw.into_inner()?;
        let payload = &self.payload;

        if payload.len() >= chunk.len() {
            self.payload.clear();
            return Ok(false);
        }

        let mut header = [0u8; 9];
        header[0] = (is_final as u8) | (1 << 1); // type = LZ, bit0 = final
        header[1..5].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        header[5..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.out.write_all(&header)?;
        self.out.write_all(payload)?;
        self.out_count += header.len() as u64 + payload.len() as u64;
        self.payload.clear();
        Ok(true)
    }

    fn write_raw_block(&mut self, chunk: &[u8], is_final: bool) -> Result<()> {
        let mut header = [0u8; 5];
        header[0] = is_final as u8; // type = raw, bit0 = final
        header[1..].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        self.out.write_all(&header)?;
        self.out.write_all(chunk)?;
        self.out_count += header.len() as u64 + chunk.len() as u64;
        Ok(())
    }
}

/// Convenience driver: compresses a reader into a writer in BLOCK_SIZE-sized
/// chunks. Returns input bytes, stats and the writer back.
pub fn compress_stream<R: Read, W: Write>(
    r: R,
    w: W,
    level: u8,
    name: &[u8],
) -> Result<(u64, Stats, W)> {
    let mut comp = Compressor::new(w, level, name)?;
    let mut r = r;
    let mut buf = vec![0u8; BLOCK_SIZE];
    loop {
        let mut filled = 0;
        while filled < BLOCK_SIZE {
            match r.read(&mut buf[filled..])? {
                0 => break,
                k => filled += k,
            }
        }
        let is_final = filled < BLOCK_SIZE;
        comp.compress_chunk(&buf[..filled], is_final)?;
        if is_final {
            break;
        }
    }
    let input_size = comp.total_in;
    let (w, stats) = comp.finish()?;
    Ok((input_size, stats, w))
}

/// In-memory helper used by tests and by `compress_bytes`.
pub fn compress_bytes(data: &[u8], level: u8, name: &[u8]) -> Result<Vec<u8>> {
    let (_, _, cursor) = compress_stream(
        io::Cursor::new(data),
        io::Cursor::new(Vec::with_capacity(data.len() / 2 + 64)),
        level,
        name,
    )?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompress::decompress_bytes;
    use crate::tables::WINDOW;

    fn roundtrip(data: &[u8], level: u8) {
        let packed = compress_bytes(data, level, b"test").unwrap();
        let out = decompress_bytes(&packed).unwrap();
        assert_eq!(out, data, "roundtrip failed at level {level}");
    }

    #[test]
    fn all_levels_roundtrip() {
        let mut data = Vec::new();
        for i in 0..50_000u32 {
            data.extend_from_slice(&(i % 977).to_le_bytes());
        }
        let head = data[..10_000].to_vec();
        data.extend_from_slice(&head); // long-range repeat
        for level in 0..=9 {
            roundtrip(&data, level);
        }
    }

    #[test]
    fn empty_and_tiny() {
        roundtrip(b"", 6);
        roundtrip(b"a", 6);
        roundtrip(b"ab", 6);
        roundtrip(b"abc", 6);
        roundtrip(b"abcd", 6);
    }

    #[test]
    fn raw_fallback_on_incompressible() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i as u8).wrapping_mul(181)).collect();
        let packed = compress_bytes(&data, 9, b"rand").unwrap();
        // Header (~14) + raw block header (5) + data + trailer (12)
        assert!(packed.len() <= data.len() + 64, "raw fallback grew too much");
        roundtrip(&data, 9);
    }

    #[test]
    fn multi_block_stream() {
        let mut data = vec![0u8; 3 * BLOCK_SIZE + 123];
        data.extend_from_slice(b"tail");
        roundtrip(&data, 6);
    }

    #[test]
    fn window_boundary_match() {
        // A match exactly WINDOW bytes back is legal.
        let mut data = vec![0x5Au8; 1000];
        data.resize(WINDOW, 0);
        data.extend_from_slice(&[0x5Au8; 1000]);
        roundtrip(&data, 9);
    }
}
