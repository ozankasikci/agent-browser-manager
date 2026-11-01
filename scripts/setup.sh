#!/bin/sh
set -eu
umask 077
project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir"
cargo build --release --locked
mkdir -p bin
install -m 700 target/release/abm bin/abm
exec "$project_dir/bin/abm" init "$@"
