//! Canonical Huffman coding: length-limited code construction, canonical
//! code assignment, a puff-style decoder, and the run-length coding of code
//! lengths used to transmit the per-block tables.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::bitio::BitReader;
use crate::error::{Error, Result};
use crate::tables::{META_REPEAT_PREV, META_ZEROS_11, META_ZEROS_3};

/// Maximum code length for the token alphabets.
pub const MAX_BITS: u32 = 15;
/// Maximum code length for the meta (code-length) alphabet.
pub const META_MAX_BITS: u32 = 7;

/// Builds Huffman code lengths for `freqs`, limited to `limit` bits.
/// Unused symbols (frequency 0) get length 0.
///
/// Construction: standard bottom-up merge with a priority queue, followed by
/// clamping to `limit` and a Kraft-inequality repair pass that pushes the
/// least-frequent (deepest) symbols further down until the code fits.
pub fn build_lengths(freqs: &[u32], limit: u32) -> Vec<u8> {
    debug_assert!((1..=31).contains(&limit));
    let n = freqs.len();
    let mut lens = vec![0u8; n];

    let mut heap: BinaryHeap<(Reverse<u64>, Reverse<u32>)> = BinaryHeap::new();
    for (i, &f) in freqs.iter().enumerate() {
        if f > 0 {
            heap.push((Reverse(f as u64), Reverse(i as u32)));
        }
    }
    match heap.len() {
        0 => return lens,
        1 => {
            let (_, Reverse(sym)) = heap.pop().unwrap();
            lens[sym as usize] = 1;
            return lens;
        }
        _ => {}
    }

    // Merge nodes; leaves occupy ids 0..n, synthetic nodes n.. (their weights
    // are appended to `weights` in id order).
    let mut weights: Vec<u64> = freqs.iter().map(|&f| f as u64).collect();
    let mut parent = vec![u32::MAX; 2 * n];
    let mut next = n as u32;
    while heap.len() > 1 {
        let (Reverse(fa), Reverse(a)) = heap.pop().unwrap();
        let (Reverse(fb), Reverse(b)) = heap.pop().unwrap();
        let nf = fa + fb;
        weights.push(nf);
        parent[a as usize] = next;
        parent[b as usize] = next;
        heap.push((Reverse(nf), Reverse(next)));
        next += 1;
    }

    for i in 0..n {
        if freqs[i] == 0 {
            continue;
        }
        let mut depth = 0u32;
        let mut cur = i as u32;
        while parent[cur as usize] != u32::MAX {
            cur = parent[cur as usize];
            depth += 1;
        }
        lens[i] = depth as u8;
    }

    clamp_and_repair(&mut lens, limit);
    lens
}

fn clamp_and_repair(lens: &mut [u8], limit: u32) {
    for l in lens.iter_mut() {
        if *l as u32 > limit {
            *l = limit as u8;
        }
    }
    let one: u64 = 1 << limit;
    let mut kraft: u64 = lens
        .iter()
        .filter(|&&l| l > 0)
        .map(|&l| 1u64 << (limit - l as u32))
        .sum();
    // Terminates: if every used symbol sat at `limit`, kraft would equal the
    // symbol count, which is < 2^limit for all alphabets in this format.
    while kraft > one {
        let mut best: Option<usize> = None;
        for (i, &l) in lens.iter().enumerate() {
            if l > 0 && (l as u32) < limit {
                match best {
                    None => best = Some(i),
                    Some(b) => {
                        if l > lens[b] {
                            best = Some(i)
                        }
                    }
                }
            }
        }
        let i = best.expect("kraft overflow with a saturated code");
        kraft -= 1u64 << (limit - lens[i] as u32 - 1);
        lens[i] += 1;
    }
}

/// Assigns canonical codes: codes of equal length are consecutive, ordered by
/// symbol value. Returns one code per symbol (0 for unused symbols).
pub fn canonical_codes(lens: &[u8]) -> Vec<u16> {
    let max = lens.iter().copied().max().unwrap_or(0) as usize;
    let mut bl_count = vec![0u16; max + 1];
    for &l in lens {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = vec![0u16; max + 2];
    let mut code = 0u16;
    for bits in 1..=max {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next_code[l as usize];
                next_code[l as usize] += 1;
                c
            }
        })
        .collect()
}

/// Decoding side: symbol counts per length plus symbols sorted by
/// (length, symbol), matching the canonical assignment.
pub struct DecodeTable {
    counts: Vec<u16>,
    symbols: Vec<u16>,
}

impl DecodeTable {
    /// Validates the code (Kraft inequality, at least one symbol) and builds
    /// the decoder.
    pub fn new(lens: &[u8]) -> Result<DecodeTable> {
        let max = lens.iter().copied().max().unwrap_or(0) as usize;
        let mut counts = vec![0u16; max + 1];
        let mut kraft: u64 = 0;
        let mut total = 0u64;
        for &l in lens {
            if l > 0 {
                counts[l as usize] += 1;
                kraft += 1u64 << (MAX_BITS - l as u32);
                total += 1;
            }
        }
        if total == 0 {
            return Err(Error::Corrupt("Huffman table with no codes"));
        }
        if kraft > 1 << MAX_BITS {
            return Err(Error::Corrupt("oversubscribed Huffman table"));
        }
        let mut symbols: Vec<u16> = (0..lens.len() as u16)
            .filter(|&s| lens[s as usize] > 0)
            .collect();
        symbols.sort_by_key(|&s| (lens[s as usize], s));
        Ok(DecodeTable { counts, symbols })
    }

    /// Decodes one symbol (the classic counts-per-length canonical walk).
    #[inline]
    pub fn decode(&self, br: &mut BitReader) -> Result<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..self.counts.len() {
            code |= br.read_bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Corrupt("invalid Huffman code"))
    }
}

/// Run-length encodes a sequence of code lengths into meta-alphabet symbols
/// with their extra-bit payloads.
///
/// Rules: runs of zeros use `META_ZEROS_3` (3..=10) or `META_ZEROS_11`
/// (11..=138); any value repeats itself once literally, then further copies
/// go through `META_REPEAT_PREV` in chunks of 3..=6; leftovers < 3 are
/// emitted literally.
pub fn rle_code_lengths(lens: &[u8]) -> Vec<(u16, u32)> {
    let mut out = Vec::new();
    let n = lens.len();
    let mut i = 0;
    while i < n {
        let v = lens[i];
        let mut run = 1;
        while i + run < n && lens[i + run] == v {
            run += 1;
        }
        i += run;
        if v == 0 {
            while run >= 3 {
                let chunk = run.min(138);
                if chunk <= 10 {
                    out.push((META_ZEROS_3, (chunk - 3) as u32));
                } else {
                    out.push((META_ZEROS_11, (chunk - 11) as u32));
                }
                run -= chunk;
            }
            for _ in 0..run {
                out.push((0, 0));
            }
        } else {
            out.push((v as u16, 0));
            run -= 1;
            while run >= 3 {
                let chunk = run.min(6);
                out.push((META_REPEAT_PREV, (chunk - 3) as u32));
                run -= chunk;
            }
            for _ in 0..run {
                out.push((v as u16, 0));
            }
        }
    }
    out
}

/// Decodes `total` code lengths from the meta-alphabet stream.
pub fn unrle_code_lengths(
    total: usize,
    meta: &DecodeTable,
    br: &mut BitReader,
) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(total);
    while out.len() < total {
        let sym = meta.decode(br)?;
        match sym {
            0..=15 => out.push(sym as u8),
            META_REPEAT_PREV => {
                let last = *out
                    .last()
                    .ok_or(Error::Corrupt("length repeat with no previous length"))?;
                let n = 3 + br.read_bits(2)?;
                for _ in 0..n {
                    out.push(last);
                }
            }
            META_ZEROS_3 => {
                let n = 3 + br.read_bits(3)?;
                out.resize(out.len() + n as usize, 0);
            }
            META_ZEROS_11 => {
                let n = 11 + br.read_bits(7)?;
                out.resize(out.len() + n as usize, 0);
            }
            _ => return Err(Error::Corrupt("invalid code-length symbol")),
        }
        if out.len() > total {
            return Err(Error::Corrupt("code length run overflows table"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitio::BitWriter;
    use crate::tables::META_ALPHABET;

    fn roundtrip(lens: &[u8], _limit: u32) {
        let codes = canonical_codes(lens);
        let mut w = BitWriter::new(Vec::new());
        for (i, &l) in lens.iter().enumerate() {
            if l > 0 {
                w.write_bits(codes[i] as u32, l as u32).unwrap();
            }
        }
        w.byte_align().unwrap();
        let bytes = w.into_inner().unwrap();

        let table = DecodeTable::new(lens).unwrap();
        let mut r = BitReader::new(&bytes);
        for (i, &l) in lens.iter().enumerate() {
            if l > 0 {
                assert_eq!(table.decode(&mut r).unwrap(), i as u16);
            }
        }
    }

    #[test]
    fn rfc1951_example_codes() {
        let lens = vec![3, 3, 3, 3, 3, 2, 4, 4]; // A..H
        let codes = canonical_codes(&lens);
        let want = [0b010, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111];
        assert_eq!(codes, want);
        roundtrip(&lens, MAX_BITS);
    }

    #[test]
    fn huffman_respects_limit_under_fibonacci_weights() {
        // Fibonacci-weighted frequencies force deep trees; the limiter must
        // bring every code within the limit while keeping Kraft <= 1.
        let n = 26;
        let mut freqs = vec![0u32; n];
        let (mut a, mut b) = (1u64, 1u64);
        for f in freqs.iter_mut() {
            *f = a as u32;
            let t = a + b;
            a = b;
            b = t;
        }
        for &limit in &[7u32, 15] {
            let lens = build_lengths(&freqs, limit);
            let kraft: u64 = lens
                .iter()
                .filter(|&&l| l > 0)
                .map(|&l| 1u64 << (limit - l as u32))
                .sum();
            assert!(kraft <= 1 << limit, "kraft violated at limit {limit}");
            assert!(lens.iter().all(|&l| l as u32 <= limit));
            roundtrip(&lens, limit);
        }
    }

    #[test]
    fn single_symbol_gets_length_one() {
        let lens = build_lengths(&[0, 0, 5, 0], MAX_BITS);
        assert_eq!(lens, vec![0, 0, 1, 0]);
        roundtrip(&lens, MAX_BITS);
    }

    #[test]
    fn skewed_frequencies_produce_shorter_codes() {
        let mut freqs = vec![1u32; 32];
        freqs[0] = 1_000_000;
        let lens = build_lengths(&freqs, MAX_BITS);
        assert_eq!(lens[0], 1);
        assert!(lens.iter().all(|&l| l > 0));
        roundtrip(&lens, MAX_BITS);
    }

    #[test]
    fn rle_roundtrip() {
        let lens: Vec<u8> = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 5, 5, 5, 5, 5, 5, 0, 0, 1, 1];
        let packed = rle_code_lengths(&lens);
        let meta_freqs = freqs_of(&packed);
        let meta_lens = build_lengths(&meta_freqs, META_MAX_BITS);
        let meta_codes = canonical_codes(&meta_lens);

        let mut w = BitWriter::new(Vec::new());
        for (sym, extra) in &packed {
            let l = meta_lens[*sym as usize] as u32;
            w.write_bits(meta_codes[*sym as usize] as u32, l).unwrap();
            let extra_bits = extra_bits_for(*sym);
            if extra_bits > 0 {
                w.write_bits(*extra, extra_bits).unwrap();
            }
        }
        w.byte_align().unwrap();
        let bytes = w.into_inner().unwrap();

        let meta = DecodeTable::new(&meta_lens).unwrap();
        let mut r = BitReader::new(&bytes);
        let out = unrle_code_lengths(lens.len(), &meta, &mut r).unwrap();
        assert_eq!(out, lens);
    }

    fn freqs_of(packed: &[(u16, u32)]) -> Vec<u32> {
        let mut f = vec![0u32; META_ALPHABET];
        for (sym, _) in packed {
            f[*sym as usize] += 1;
        }
        f
    }

    fn extra_bits_for(sym: u16) -> u32 {
        match sym {
            META_REPEAT_PREV => 2,
            META_ZEROS_3 => 3,
            META_ZEROS_11 => 7,
            _ => 0,
        }
    }
}
