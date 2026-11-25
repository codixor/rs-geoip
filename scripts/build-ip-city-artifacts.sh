#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INPUT_JSON="${1:-$ROOT/ip-city-merged.json}"
OUTPUT_LZ4="${2:-$ROOT/ip-city-packed.lz4}"
OUTPUT_BIN="${3:-$ROOT/ip-city-packed-paged.bin}"
BLOCK_SIZE="${BLOCK_SIZE:-256}"
CARGO="${CARGO:-cargo}"

"$CARGO" run --manifest-path "$ROOT/Cargo.toml" --features city,build \
  --bin build_ip_city_pack -- \
  --input-json "$INPUT_JSON" \
  --output-lz4 "$OUTPUT_LZ4"

"$CARGO" run --manifest-path "$ROOT/Cargo.toml" --features city,build \
  --bin build_ip_city_pack_paged -- \
  --input-lz4 "$OUTPUT_LZ4" \
  --output-bin "$OUTPUT_BIN" \
  --block-size "$BLOCK_SIZE"
