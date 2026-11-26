#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# == 0 ]]; then
  set -- country city country,build city,build builtin
fi

for features in "$@"; do
  args=(--locked --features "$features")
  if [[ "$features" != city ]]; then
    args+=(--no-default-features)
  fi
  cargo clippy --all-targets "${args[@]}" -- -D warnings
  cargo test "${args[@]}"
done
