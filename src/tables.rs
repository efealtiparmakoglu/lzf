//! LZF1 symbol alphabets: literal/length codes, distance codes and the
//! code-length meta alphabet. All tables are format constants — see
//! FORMAT.md for the rationale.

pub const LITLEN_ALPHABET: usize = 288; // 256 literals + 32 length codes
pub const DIST_ALPHABET: usize = 32;
pub const META_ALPHABET: usize = 19;

pub const MIN_MATCH: usize = 4;
pub const MAX_MATCH: usize = 512;
pub const WINDOW: usize = 1 << 16; // 64 KiB sliding window per block

/// Length code i covers `LENGTH_BASES[i] ..= LENGTH_BASES[i] + (1 <<
/// LENGTH_EXTRA[i]) - 1`. Codes 0..=8 are the plain lengths 4..=12.
pub const LENGTH_BASES: [u16; 32] = [
    4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 17, 19, 21, 25, 29, 33, 37, 45, 53, 61, 69, 85, 101,
    117, 133, 165, 197, 229, 261, 325, 389,
];
pub const LENGTH_EXTRA: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6,
    7,
];

/// Distance code i covers `DIST_BASES[i] ..= DIST_BASES[i] + (1 <<
/// DIST_EXTRA[i]) - 1`. Code 31 tops out at 65536.
pub const DIST_BASES: [u16; 32] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577, 32769, 49153,
];
pub const DIST_EXTRA: [u8; 32] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13, 14, 14,
];

/// Meta alphabet symbols >= 16 (run-length codes for code lengths).
pub const META_REPEAT_PREV: u16 = 16; // 3..=6 repeats, 2 extra bits
pub const META_ZEROS_3: u16 = 17; // 3..=10 zeros, 3 extra bits
pub const META_ZEROS_11: u16 = 18; // 11..=138 zeros, 7 extra bits

/// Maps a match length (4..=516) to its symbol (256..=287) and extra bits.
pub fn length_to_symbol(len: usize) -> (u16, u32) {
    debug_assert!((MIN_MATCH..=516).contains(&len));
    let idx = LENGTH_BASES.partition_point(|&b| (b as usize) <= len) - 1;
    let extra_val = (len - LENGTH_BASES[idx] as usize) as u32;
    (256 + idx as u16, extra_val)
}

/// Inverse of [`length_to_symbol`]: symbol to (base length, extra bit count).
pub fn symbol_to_length(sym: u16) -> (usize, u32) {
    let idx = (sym - 256) as usize;
    (LENGTH_BASES[idx] as usize, LENGTH_EXTRA[idx] as u32)
}

/// Maps a distance (1..=65536) to its symbol and extra bits.
pub fn dist_to_symbol(dist: usize) -> (u16, u32) {
    debug_assert!((1..=65536).contains(&dist));
    let idx = DIST_BASES.partition_point(|&b| (b as usize) <= dist) - 1;
    let extra_val = (dist - DIST_BASES[idx] as usize) as u32;
    (idx as u16, extra_val)
}

/// Inverse of [`dist_to_symbol`]: symbol to (base distance, extra bit count).
pub fn symbol_to_dist(sym: u16) -> (usize, u32) {
    let idx = sym as usize;
    (DIST_BASES[idx] as usize, DIST_EXTRA[idx] as u32)
}

/// Extra-bit count for a literal/length symbol (256..=287).
#[inline]
pub fn symbol_to_length_extra(sym: u16) -> u32 {
    LENGTH_EXTRA[(sym - 256) as usize] as u32
}

/// Extra-bit count for a distance symbol (0..=31).
#[inline]
pub fn symbol_to_dist_extra(sym: u16) -> u32 {
    DIST_EXTRA[sym as usize] as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_tables_are_consistent() {
        // Coverage must be gapless and monotonic across the full range.
        for sym in 0u16..32 {
            let (base, extra) = symbol_to_length(256 + sym);
            assert_eq!(base as u16, LENGTH_BASES[sym as usize]);
            assert_eq!(extra as u8, LENGTH_EXTRA[sym as usize]);
        }
        for len in MIN_MATCH..=516 {
            let (sym, extra_val) = length_to_symbol(len);
            assert!((256..256 + 32).contains(&sym));
            let (base, extra) = symbol_to_length(sym);
            assert!(extra_val < 1 << extra, "extra bits overflow for len {len}");
            assert_eq!(base + extra_val as usize, len);
        }
    }

    #[test]
    fn dist_tables_are_consistent() {
        for dist in 1..=65536usize {
            let (sym, extra_val) = dist_to_symbol(dist);
            assert!(sym < 32);
            let (base, extra) = symbol_to_dist(sym);
            assert!(extra_val < 1 << extra, "extra bits overflow for dist {dist}");
            assert_eq!(base + extra_val as usize, dist);
        }
    }

    #[test]
    fn table_sizes() {
        assert_eq!(LENGTH_BASES.len(), 32);
        assert_eq!(LENGTH_EXTRA.len(), 32);
        assert_eq!(DIST_BASES.len(), 32);
        assert_eq!(DIST_EXTRA.len(), 32);
        // Last distance code must reach the full 64 KiB window.
        let (base, extra) = symbol_to_dist(31);
        assert_eq!(base + (1 << extra) - 1, 65536);
        // Last length code must cover MAX_MATCH.
        let (base, extra) = symbol_to_length(287);
        assert!(base + (1 << extra) > MAX_MATCH);
    }
}
