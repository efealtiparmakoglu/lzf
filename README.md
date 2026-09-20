# lzf

**Fast, zero-dependency file compression in pure Rust — LZ77 + canonical Huffman, packed in its own container format (LZF1).**

[![CI](https://github.com/efealtiparmakoglu/lzf/actions/workflows/ci.yml/badge.svg)](https://github.com/efealtiparmakoglu/lzf/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)

A fast, **zero-dependency** file compressor written in pure Rust. LZ77 with
lazy matching plus per-block canonical Huffman coding, packed into its own
container format (**LZF1**). Think "gzip's algorithm, my own file format, no
crates anywhere in the tree".

```
$ lzf c enwik-txt
enwik-txt.lzf: 100.0 MiB -> 33.7 MiB (66.3% saved) in 1.62 s [61.7 MiB/s, level 6]
$ lzf d enwik-txt.lzf
enwik-txt.lzf -> enwik-txt: 100.0 MiB in 0.91 s [109.5 MiB/s, crc32 ok, stored as "enwik-txt"]
```

## Why lzf

- **Zero dependencies** — the entire codec (bit I/O, Huffman construction
  with a 15-bit limiter, hash-chain match finder, CRC-32) is hand-written on
  top of `std`. `Cargo.toml` has an empty `[dependencies]` section.
- **Self-contained blocks** — every 1 MiB block carries its own Huffman
  tables and resets its own 64 KiB window. Corruption cannot cascade across
  blocks, and the design leaves the door open to parallel decode.
- **Integrity by construction** — every file ends with a CRC-32 trailer;
  `lzf d` and `lzf t` verify it. Malformed input is always reported as an
  error, never a panic.
- **Never bigger than the input** — blocks that don't compress are stored
  raw, so the worst case is input size + a handful of framing bytes.
- **Evergreen** — the format depends on no external service, API, or
  moving-target library; LZF1 files written today decode identically forever.

## Install

```sh
cargo install --path .
```

Requires Rust 1.70+. No other dependencies, build steps, or network access.

## Usage

```sh
lzf compress [OPTIONS] <FILE>...    # aliases: c, z
lzf decompress [OPTIONS] <FILE>...  # aliases: d, x
lzf test <FILE>...                  # aliases: t — verify CRC without writing
```

| Option | Meaning |
|---|---|
| `-o <FILE>` | output path (single input only) |
| `-0 .. -9`, `--level <N>` | compression level (default 6; 0 = store) |
| `-f`, `--force` | overwrite existing outputs |
| `--rm` | remove input files after a successful run |
| `-q`, `--quiet` | suppress progress output |
| `-h`, `-V` | help / version |

Examples:

```sh
lzf c -9 backup.tar          # best compression
lzf c *.log --rm             # compress a batch, drop originals on success
lzf d backup.tar.lzf         # CRC-verified decompression
lzf t important.lzf          # integrity check only
```

Exit codes: `0` success, `1` runtime failure (I/O, corrupt input, existing
output), `2` usage error.

## The format in one page

A `.lzf` file is:

```
"LZF1" magic | version=1 | name_len | name | block* | size(u64 LE) | crc32(u32 LE)
```

Each block is either **raw** (`len` + stored bytes) or **LZ**: an independent
chunk of up to 1 MiB encoded as literal/length symbols and distance symbols
with two canonical Huffman tables, which are themselves transmitted through a
third, 7-bit-limited table with run-length-coded lengths. Bit packing is
MSB-first. Distances reach back at most 64 KiB; match lengths reach 4–512
bytes; the last block carries a final flag.

The complete bit-level specification lives in [FORMAT.md](FORMAT.md).

## Algorithm

- **Match finding**: 4-byte rolling hash into 128 Ki hash buckets, hash
  chains over a 64 KiB window, gzip-style one-step lazy matching, with
  `nice_len` early exit and chain depth scaled by level (1–9).
- **Entropy coding**: canonical Huffman per block; code lengths limited to
  15 bits (7 for the meta table) via clamping + a Kraft-inequality repair
  pass that pushes rare symbols deeper. The classic counts-per-length loop
  decodes without a tree.
- **Degenerate cases**: empty files, single-byte files, incompressible data,
  and exact block-size boundaries all have dedicated tests.

## Performance

Release build, Apple silicon (M-series), `scripts/bench.sh` corpora. Sizes in
parentheses are the compressed totals; gzip is macOS `gzip -6` for context.
On run-heavy repetitive text, lzf's 512-byte match cap beats gzip's 258-byte
cap by a wide margin; on already-compressed data both fall back to storing.

| Corpus | Input | lzf -1 | lzf -6 | lzf -9 | gzip -6 |
|---|---|---|---|---|---|
| repetitive text | 9.1 MiB | 176.6 KiB @ 204 MiB/s | 137.3 KiB @ 78 MiB/s | **86.6 KiB** @ 24 MiB/s | 486.4 KiB |
| random binary | 8.0 MiB | 8.0 MiB (+73 B) | 8.0 MiB (+73 B) | 8.0 MiB (+73 B) | 8.0 MiB (+2.4 KiB) |
| zeros + random mix | 8.0 MiB | 4.0 MiB | 4.0 MiB | 4.0 MiB | 3.9 MiB |

Decompression runs at ~270–300 MiB/s regardless of level and is CRC-verified
end to end. Round-trips are byte-identical.

## Development

```sh
cargo test            # unit + integration suites
cargo test --release  # same, compiled for speed
cargo build --release # the lzf binary
```

The test suite covers: CRC vectors, bit-level roundtrips, canonical-code
examples from RFC 1951, Fibonacci-weighted Huffman tables (the pathological
case for any length limiter), table↔symbol parity across every legal length
and distance, multi-block streams, corruption and truncation handling, and a
full CLI workflow.

## License

MIT — see [LICENSE](LICENSE).
