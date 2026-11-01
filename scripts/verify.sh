#!/bin/sh
set -eu
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
if [ "${1:-}" = "--chrome" ]; then
    cargo test --locked --test live_chrome -- --ignored --nocapture --test-threads=1
elif [ "$#" -gt 0 ]; then
    printf '%s\n' 'Usage: scripts/verify.sh [--chrome]' >&2
    exit 2
fi
