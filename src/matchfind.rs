//! Hash-chain match finder with lazy (one-step lookahead) matching, in the
//! spirit of gzip's deflate but tuned for this format: matches are 4..=512
//! bytes at distances up to 64 KiB, and chains never cross block boundaries
//! because each block resets the window.

use crate::tables::{MAX_MATCH, MIN_MATCH, WINDOW};

pub const HASH_BITS: u32 = 17;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MULT: u32 = 0x9E37_79B1;

/// Search effort knobs derived from the CLI level (0..=9).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub max_chain: u32,
    pub nice_len: usize,
    pub lazy: bool,
}

pub fn params(level: u8) -> Params {
    match level {
        0 => Params { max_chain: 0, nice_len: 0, lazy: false },
        1 => Params { max_chain: 4, nice_len: 12, lazy: false },
        2 => Params { max_chain: 8, nice_len: 16, lazy: false },
        3 => Params { max_chain: 16, nice_len: 24, lazy: false },
        4 => Params { max_chain: 24, nice_len: 32, lazy: true },
        5 => Params { max_chain: 40, nice_len: 48, lazy: true },
        6 => Params { max_chain: 80, nice_len: 96, lazy: true },
        7 => Params { max_chain: 160, nice_len: 192, lazy: true },
        8 => Params { max_chain: 320, nice_len: 384, lazy: true },
        _ => Params { max_chain: 1024, nice_len: MAX_MATCH, lazy: true },
    }
}

/// Chain state for one block. Positions are block-local.
pub struct MatchFinder {
    head: Vec<i32>,
    prev: Vec<i32>,
}

impl MatchFinder {
    pub fn new(block_capacity: usize) -> Self {
        MatchFinder {
            head: vec![-1; HASH_SIZE],
            prev: vec![-1; block_capacity],
        }
    }

    /// Drops all chains (called at every block boundary; `prev` entries are
    /// only reachable through `head`, so resetting `head` suffices).
    pub fn reset(&mut self) {
        self.head.fill(-1);
    }

    #[inline]
    fn hash4(data: &[u8], pos: usize) -> usize {
        let v = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        (v.wrapping_mul(HASH_MULT) >> (32 - HASH_BITS)) as usize
    }

    /// Registers `pos` in the chain for its 4-byte hash. No-op when fewer
    /// than 4 bytes remain.
    #[inline]
    pub fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + MIN_MATCH > data.len() {
            return;
        }
        let h = Self::hash4(data, pos);
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    /// Inserts `pos`, then searches the chain for the longest match starting
    /// at `pos`. Returns `(length, distance)` or `None`.
    pub fn insert_and_find(
        &mut self,
        data: &[u8],
        pos: usize,
        params: &Params,
    ) -> Option<(usize, u32)> {
        if pos + MIN_MATCH > data.len() {
            return None;
        }
        let h = Self::hash4(data, pos);
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;

        let cur = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let limit = (data.len() - pos).min(MAX_MATCH);
        let max_dist = pos.min(WINDOW);
        let mut best_len = MIN_MATCH - 1;
        let mut best: Option<(usize, u32)> = None;
        let mut chain = params.max_chain;
        let mut cand = self.prev[pos];

        while cand >= 0 && chain > 0 {
            chain -= 1;
            let c = cand as usize;
            let dist = pos - c;
            if dist > max_dist {
                break; // chain positions only move backwards
            }
            if u32::from_le_bytes([data[c], data[c + 1], data[c + 2], data[c + 3]]) == cur {
                let l = match_len(data, c, pos, limit);
                if l > best_len {
                    best_len = l;
                    best = Some((l, dist as u32));
                    if l >= params.nice_len || l >= limit {
                        break;
                    }
                }
            }
            cand = self.prev[c];
        }
        best
    }
}

/// Length of the common prefix of `data[a..]` and `data[b..]`, capped at
/// `limit`. Requires `a < b` and `b + limit <= data.len()`.
#[inline]
fn match_len(data: &[u8], a: usize, b: usize, limit: usize) -> usize {
    let mut k = 0;
    while k + 8 <= limit && data[a + k..a + k + 8] == data[b + k..b + k + 8] {
        k += 8;
    }
    while k < limit && data[a + k] == data[b + k] {
        k += 1;
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_all(data: &[u8], level: u8) -> Vec<Option<(usize, u32)>> {
        let p = params(level);
        let mut mf = MatchFinder::new(data.len());
        (0..data.len())
            .map(|pos| mf.insert_and_find(data, pos, &p))
            .collect()
    }

    #[test]
    fn finds_exact_repeat() {
        let mut data = vec![7u8; 64];
        data.extend_from_slice(&[7u8; 64]);
        let hits = find_all(&data, 6);
        // From position 64 on, the whole tail matches; the nearest candidate
        // (distance 1, the self-overlap) is found first.
        assert_eq!(hits[64], Some((64, 1)));
        assert_eq!(hits[100], Some((28, 1)));
    }

    #[test]
    fn self_overlapping_run() {
        let data = vec![42u8; 300];
        let hits = find_all(&data, 6);
        // Self-overlap compresses any run: at pos 4 the whole rest matches.
        assert_eq!(hits[4], Some((296, 1)));
    }

    #[test]
    fn no_match_in_random_prefix() {
        let mut data = vec![0x1Bu8; 100];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x5A; // poor-man's varied prefix
        }
        let twin = data[..50].to_vec();
        data.extend_from_slice(&twin);
        let hits = find_all(&data, 9); // deep chain so the twin is reached
        // The last 3 positions have < MIN_MATCH bytes left: no match there.
        let found = hits.iter().skip(100).take(47).flatten().count();
        assert_eq!(found, 47);
    }

    #[test]
    fn window_is_enforced() {
        // Repeat data that is farther away than WINDOW: must NOT be found.
        let far = WINDOW + 4096;
        let mut data = vec![0xABu8; 1000];
        data.resize(far, 0);
        data.extend_from_slice(&[0xABu8; 1000]);
        let hits = find_all(&data, 6);
        for hit in hits.iter().skip(far).flatten() {
            assert!(hit.1 as usize <= WINDOW);
        }
    }

    #[test]
    fn short_tail_returns_none() {
        let data = vec![1u8, 2, 3];
        let hits = find_all(&data, 9);
        assert!(hits.iter().all(|h| h.is_none()));
    }
}
