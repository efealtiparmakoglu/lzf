# The LZF1 Container Format — Specification v1

This document is the normative description of the LZF1 file format as
implemented by `lzf` 1.0. LZF1 has no external dependencies: every structure
below is defined in terms of bytes and bits, so an independent implementation
needs nothing but this document.

Conventions: multi-byte integers are **little-endian**. Bitstreams pack bits
**MSB-first** within each byte. "uN" means an unsigned N-bit integer.

## 1. File layout

```
+--------+---------+----------+------+------------+-------------+-----------+
| magic  | version | name_len | name |  block *   | orig_size   |  crc32    |
| 4 B    | 1 B     | 1 B      | var  |   var      | 8 B (u64)   | 4 B (u32) |
+--------+---------+----------+------+------------+-------------+-----------+
```

| Field | Description |
|---|---|
| magic | ASCII `"LZF1"` (0x4C 0x5A 0x46 0x31) |
| version | `0x01` |
| name_len | 0–255; length of `name` in bytes |
| name | Informational basename of the original file, raw bytes. **Consumers must not use it as a filesystem path without sanitization.** |
| block | Zero or more blocks; the final block has the final flag set |
| orig_size | Total size of the decompressed data in bytes |
| crc32 | CRC-32 (IEEE, poly 0xEDB88320 reflected) of the decompressed data |

An empty file compresses to header + one zero-length final raw block +
trailer (crc32 of empty input = 0x00000000).

## 2. Blocks

Each block starts with one header byte:

```
bit 0      : final flag (1 = last block of the file)
bits 1..7  : block type (0 = raw, 1 = LZ)
```

### 2.1 Raw block

```
type=0 | u32 raw_len | raw_len bytes of uncompressed data
```

Encoders SHOULD emit a raw block whenever the LZ encoding of the same data
would be equal or larger, guaranteeing a bounded worst case.

### 2.2 LZ block

```
type=1 | u32 raw_len | u32 payload_len | payload_len bytes (bitstream)
```

`raw_len` is the number of bytes the block decodes to (at most 1 MiB,
encoder's choice); `payload_len` is the byte length of the bitstream that
follows. The bitstream is padded with zero bits to a byte boundary at its
end. Decoders must stop decoding exactly after `raw_len` output bytes and
ignore trailing padding.

## 3. LZ block bitstream

All fields below are bit-packed, in order:

```
1.  19 × u4      : code lengths (0–15) of the meta alphabet, symbols 0..=18
2.  u9           : HLIT - 1   (number of literal/length codes, 1..=288)
3.  u6           : HDIST - 1  (number of distance codes, 1..=32)
4.  code lengths : HLIT + HDIST entries, run-length coded via the meta
                   alphabet and coded with its canonical Huffman code
5.  token data   : LZ tokens coded with the literal/length and distance
                   Huffman codes; ends when exactly raw_len bytes are produced
```

### 3.1 Alphabets

**Literal/length alphabet (288 symbols).** Symbols 0–255 are literal bytes.
Symbols 256–287 encode match lengths:

| Symbol | Base | Extra | Lengths covered |
|---|---|---|---|
| 256–264 | 4,5,6,7,8,9,10,11,12 | 0 | 4–12 |
| 265–268 | 13,15,17,19 | 1 | 13–20 |
| 269–272 | 21,25,29,33 | 2 | 21–36 |
| 273–276 | 37,45,53,61 | 3 | 37–68 |
| 277–280 | 69,85,101,117 | 4 | 69–132 |
| 281–284 | 133,165,197,229 | 5 | 133–260 |
| 285–286 | 261,325 | 6 | 261–388 |
| 287 | 389 | 7 | 389–516 |

Extra bits follow the symbol's Huffman code, plain uN MSB-first, value =
`length - base`. Encoders cap match length at 512.

**Distance alphabet (32 symbols).** Distances are byte distances backwards
into the *current block's* output only (blocks are independent):

| Symbol | Base | Extra | Distances covered |
|---|---|---|---|
| 0–3 | 1,2,3,4 | 0 | 1–4 |
| 4–5 | 5,7 | 1 | 5–8 |
| 6–7 | 9,13 | 2 | 9–16 |
| 8–9 | 17,25 | 3 | 17–32 |
| 10–11 | 33,49 | 4 | 33–64 |
| 12–13 | 65,97 | 5 | 65–128 |
| 14–15 | 129,193 | 6 | 129–256 |
| 16–17 | 257,385 | 7 | 257–512 |
| 18–19 | 513,769 | 8 | 513–1024 |
| 20–21 | 1025,1537 | 9 | 1025–2048 |
| 22–23 | 2049,3073 | 10 | 2049–4096 |
| 24–25 | 4097,6145 | 11 | 4097–8192 |
| 26–27 | 8193,12289 | 12 | 8193–16384 |
| 28–29 | 16385,24577 | 13 | 16385–32768 |
| 30–31 | 32769,49153 | 14 | 32769–65536 |

**Meta alphabet (19 symbols).** Used only to transmit code lengths:

| Symbol | Meaning | Extra |
|---|---|---|
| 0–15 | a code length of that value | 0 |
| 16 | copy the previous code length 3 + u2 times (3–6) | 2 |
| 17 | 3 + u3 zeros (3–10) | 3 |
| 18 | 11 + u7 zeros (11–138) | 7 |

"Previous code length" refers to the last length value emitted as a literal
(0–15); symbol 16 is invalid as the very first symbol of the stream.

### 3.2 Code-length run-length coding

The `HLIT + HDIST` code lengths (literal/length lengths first, then distance
lengths) are converted to a meta symbol stream as follows, left to right:

- Runs of `0`: emit `17` in chunks of 3–10, then `18` in chunks of 11–138
  (encoders should prefer `17` while the remainder allows); a remainder of
  1–2 zeros is emitted as literal `0` symbols.
- Any nonzero value `v` appearing `r` times: emit `v` literally once, then
  emit `16` in chunks of 3–6 for further copies; a remainder of 1–2 is
  emitted as literal `v` symbols.

Trailing zeros of each alphabet may be omitted by shrinking HLIT/HDIST to
the last nonzero length (+1), with a minimum of 1. The literal/length table
must contain at least one code; the distance table must contain at least one
code even for blocks with no matches (any single code works; it is never
used).

### 3.3 Canonical Huffman codes

Codes are canonical (RFC 1951 style): within a length, codes are consecutive
and ordered by symbol value; shorter lengths precede longer ones:

```
bl_count[b]   = number of codes of length b
code = 0; for b in 1..=max: { code = (code + bl_count[b-1]) << 1; next[b] = code }
```

Codes are transmitted and read MSB-first (the first bit of a code is its
most significant bit). Decoders verify the Kraft inequality
(Σ 2^(-len) ≤ 1); oversubscribed tables are a format violation.

Encoder limits: token tables use at most 15-bit codes, the meta table at
most 7-bit codes.

### 3.4 Token stream

Repeat until the output reaches `raw_len`:

- Decoding a literal/length symbol `s < 256`: append byte `s`.
- `s ≥ 256`: read the length (`base + extra`), decode a distance symbol from
  the distance table, read its extra bits, then copy `length` bytes from
  `distance` bytes back. Distances may exceed the current copy length
  (self-overlapping copies expand runs byte-by-byte). Decoders must reject
  distances that reach before the start of the block's output or exceed
  65536, matches that overrun `raw_len`, invalid symbols, and bitstream
  exhaustion — these are all corruption, not panics.

## 4. Versioning

Version byte allows future format changes; this document describes version
1 only. Any change to the layout requires a version bump. The name field,
block sizing (1 MiB max raw), the 64 KiB distance window and the 512-byte
match cap are format constants for v1.

## 5. Test vectors

The reference implementation's test suite includes: CRC-32 of `""` →
`0x00000000` and `"123456789"` → `0xCBF43926`; RFC 1951's canonical-code
example; and round-trips over empty, single-byte, run-heavy, random, and
multi-megabyte inputs at every encoder level.
