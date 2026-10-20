#!/usr/bin/env bash
# Run all checks serially.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --workspace --lib --tests --examples -- -D warnings
cargo test --workspace --lib --tests
cargo test --lib --tests --no-default-features --features frame -p lz4rip
for package in lz4rip lz4rip-encode lz4rip-decode; do
    cargo check --no-default-features -p "$package"
done
