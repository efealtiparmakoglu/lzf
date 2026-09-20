#!/bin/sh
# Reproducible micro-benchmark for the README's performance table.
# Usage: scripts/bench.sh   (writes corpora into bench-out/, gitignored)
set -eu
cd "$(dirname "$0")/.."
OUT="bench-out"
mkdir -p "$OUT"

# Corpus 1: repetitive text (the codec's happy place)
awk 'BEGIN { for (i = 0; i < 200000; i++) print "the quick brown fox jumps over the lazy dog", i % 997 }' > "$OUT/text.txt"

# Corpus 2: pseudo-random binary (incompressible)
head -c 8388608 /dev/urandom > "$OUT/random.bin"

# Corpus 3: half long zero runs, half random
{ head -c 4194304 /dev/zero; head -c 4194304 /dev/urandom; } > "$OUT/zeros.bin"

BIN=target/release/lzf
cargo build --release >/dev/null

for f in "$OUT/text.txt" "$OUT/random.bin" "$OUT/zeros.bin"; do
    for lvl in 1 6 9; do
        "$BIN" c "-$lvl" -f "$f"
    done
    "$BIN" d -f -o "$f.restored" "$f.lzf"
    cmp "$f" "$f.restored" && echo "$(basename "$f"): roundtrip byte-identical"
    "$BIN" t "$f.lzf"
done

echo "--- gzip -6 size comparison ---"
for f in "$OUT/text.txt" "$OUT/random.bin" "$OUT/zeros.bin"; do
    gzip -6 -c "$f" > "$f.gz"
    printf '%s  input: %s  lzf -6: %s  gzip -6: %s\n' "$(basename "$f")" \
        "$(stat -f%z "$f")" "$(stat -f%z "$f.lzf")" "$(stat -f%z "$f.gz")"
done
