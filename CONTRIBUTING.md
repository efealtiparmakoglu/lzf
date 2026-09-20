# Contributing to lzf

Thanks for your interest! lzf aims to stay small, sharp and dependency-free.

## Ground rules

- **Zero dependencies.** `Cargo.toml` must keep an empty `[dependencies]`
  section. Everything lives on `std` — that is the point of the project.
- **The format is normative.** [FORMAT.md](FORMAT.md) describes LZF1 v1.
  Changes to the wire format require a version bump and a spec update in the
  same change.
- **No panics on input.** Any malformed or truncated input must surface as
  `Error::Corrupt`, never as a panic. Fuzz-thinking is welcome.

## Workflow

```sh
cargo test              # everything must pass
cargo test --release
cargo clippy --all-targets   # zero warnings
./scripts/bench.sh      # if you touched the codec, show the numbers
```

## Pull requests

1. Keep PRs focused: one feature or fix each.
2. If you change compression or decompression behavior, include a round-trip
   test and, for ratio changes, before/after numbers from `scripts/bench.sh`.
3. Match the existing style; `cargo fmt` output is the reference.

## Reporting bugs

Open an issue with: the `lzf` version, the exact command line, and — if
possible — the smallest input that reproduces the problem. Corrupt-input
reports are especially valuable: describe how the file got damaged.
