//! End-to-end integration tests: real-world-shaped corpora, block
//! boundaries, every compression level, corruption handling and the CLI.

use std::fs;
use std::process::ExitCode;

use lzf::compress::compress_bytes;
use lzf::decompress::decompress_bytes;
use lzf::format::BLOCK_SIZE;

/// Deterministic xorshift64* PRNG so failures reproduce.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next_u64() >> 24) as u8
    }
}

fn roundtrip(data: &[u8], level: u8, label: &str) {
    let packed = compress_bytes(data, level, label.as_bytes()).unwrap();
    let out = decompress_bytes(&packed).unwrap();
    assert_eq!(out, data, "roundtrip mismatch: {label} at level {level}");
}

#[test]
fn empty_and_edge_sizes() {
    for level in [0u8, 1, 6, 9] {
        roundtrip(b"", level, "empty");
        roundtrip(b"x", level, "one");
        roundtrip(&[0u8; 3], level, "three");
        roundtrip(b"abcd", level, "min-match");
    }
}

#[test]
fn all_levels_on_structured_data() {
    // Word salad with repetition — the regime LZ77+Huffman is built for.
    let mut rng = Rng::new(0x5EED_0001);
    let words: Vec<Vec<u8>> = (0..300)
        .map(|_| (0..rng.next_u64() % 9 + 3).map(|_| b'a' + rng.byte() % 26).collect())
        .collect();
    let mut data = Vec::new();
    for _ in 0..60_000 {
        data.extend_from_slice(&words[(rng.next_u64() % 300) as usize]);
        data.push(b' ');
    }
    for level in 0..=9u8 {
        roundtrip(&data, level, "words");
    }
    // Structured data must actually compress at the default level.
    let packed = compress_bytes(&data, 6, b"words").unwrap();
    assert!(packed.len() < data.len() / 2, "poor ratio on text-like data");
}

#[test]
fn incompressible_random_stays_small() {
    let mut rng = Rng::new(0xDEAD_BEEF);
    let data: Vec<u8> = (0..BLOCK_SIZE / 2).map(|_| rng.byte()).collect();
    let packed = compress_bytes(&data, 9, b"rand").unwrap();
    assert!(packed.len() <= data.len() + 64, "incompressible data grew");
    roundtrip(&data, 9, "rand");
}

#[test]
fn block_boundaries() {
    let mut rng = Rng::new(42);
    for n in [
        BLOCK_SIZE - 1,
        BLOCK_SIZE,
        BLOCK_SIZE + 1,
        2 * BLOCK_SIZE,
        2 * BLOCK_SIZE + 777,
    ] {
        // Half pseudo-random, half periodic: both block types exercised.
        let data: Vec<u8> = (0..n)
            .map(|i| {
                if i % 4096 < 2048 {
                    rng.byte()
                } else {
                    (i % 251) as u8
                }
            })
            .collect();
        roundtrip(&data, 6, &format!("boundary-{n}"));
    }
}

#[test]
fn runs_and_self_overlap() {
    let mut data = Vec::new();
    for b in 0..=255u8 {
        data.extend(std::iter::repeat(b).take((b as usize % 13) * 997 + 1));
    }
    roundtrip(&data, 9, "runs");
}

#[test]
fn long_range_match_beyond_window() {
    let mut rng = Rng::new(7);
    let a: Vec<u8> = (0..50_000).map(|_| rng.byte()).collect();
    let mut data = a.clone();
    let b: Vec<u8> = (0..300_000).map(|_| rng.byte()).collect();
    data.extend_from_slice(&b);
    data.extend_from_slice(&a); // 350_000 bytes back: beyond the 64 KiB window
    roundtrip(&data, 9, "long-range");
}

#[test]
fn pathological_fibonacci_distribution() {
    // 26 byte values with Fibonacci-ish frequencies force extreme Huffman
    // trees; the 15-bit code limiter must keep decoding valid.
    let mut freqs = [0u64; 26];
    let (mut x, mut y) = (1u64, 1u64);
    for f in freqs.iter_mut() {
        *f = x;
        let t = x + y;
        x = y;
        y = t;
    }
    let mut data = Vec::new();
    for (i, &f) in freqs.iter().enumerate() {
        for _ in 0..f.min(4000) {
            data.push(b'a' + i as u8);
        }
    }
    for level in [6u8, 9] {
        roundtrip(&data, level, "fibonacci");
    }
}

#[test]
fn every_byte_value_once() {
    let mut data: Vec<u8> = (0..=255u8).collect();
    let mut rng = Rng::new(99);
    // light shuffle
    for i in (1..data.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        data.swap(i, j);
    }
    roundtrip(&data, 6, "bytes");
}

#[test]
fn cli_end_to_end() {
    let dir = std::env::temp_dir().join(format!(
        "lzf-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();

    let orig = dir.join("doc.txt");
    let mut content = Vec::new();
    for i in 0..40_000 {
        content.extend_from_slice(format!("line {i} the quick brown fox\n").as_bytes());
    }
    fs::write(&orig, &content).unwrap();

    // compress
    let args: Vec<String> = ["c", "-f", orig.to_str().unwrap()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(lzf::run(&args), ExitCode::SUCCESS);
    let packed_path = dir.join("doc.txt.lzf");
    let packed = fs::read(&packed_path).unwrap();
    assert!(packed.starts_with(b"LZF1"));

    // test
    let args: Vec<String> = ["t", packed_path.to_str().unwrap()]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(lzf::run(&args), ExitCode::SUCCESS);

    // decompress and compare
    let restored = dir.join("doc.out.txt");
    let args: Vec<String> = [
        "d",
        "-f",
        "-o",
        restored.to_str().unwrap(),
        packed_path.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(lzf::run(&args), ExitCode::SUCCESS);
    assert_eq!(fs::read(&restored).unwrap(), content);

    // compressing again without -f must fail (output exists)
    let args: Vec<String> = ["c", orig.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    assert_eq!(lzf::run(&args), ExitCode::FAILURE);

    // garbage input to `test` must fail cleanly
    let junk = dir.join("junk.lzf");
    fs::write(&junk, b"NOT LZF AT ALL NOT LZF").unwrap();
    let args: Vec<String> = ["t", junk.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    assert_eq!(lzf::run(&args), ExitCode::FAILURE);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn header_and_trailer_layout() {
    let packed = compress_bytes(b"payload payload payload", 6, b"readme.md").unwrap();
    assert!(packed.starts_with(b"LZF1\x01"));
    assert_eq!(packed[5], 9); // name length
    assert_eq!(&packed[6..15], b"readme.md");
    // trailer: u64 original size + u32 crc32
    let n = packed.len();
    let size = u64::from_le_bytes(packed[n - 12..n - 4].try_into().unwrap());
    assert_eq!(size, 23);
}
