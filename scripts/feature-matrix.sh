#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

for features in country city country,build city,build builtin; do
  cargo test --locked --no-default-features --features "$features"
done
