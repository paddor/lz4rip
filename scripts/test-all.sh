#!/usr/bin/env bash
# Run checks serially; performance measurements must not overlap builds/tests.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --workspace --lib --tests --examples -- -D warnings
cargo clippy --workspace --lib --tests --examples --features paranoid -- -D warnings
cargo test --workspace --lib --tests
cargo test --lib --tests --no-default-features --features frame -p lz4rip
cargo test --lib --tests --features paranoid -p lz4rip
cargo test --lib --tests --no-default-features --features frame,paranoid -p lz4rip
cargo test --example perf_verify
for package in lz4rip lz4rip-encode lz4rip-decode; do
    cargo check --no-default-features -p "$package"
    cargo check --no-default-features --features paranoid -p "$package"
done

if [[ -n "${CI:-}" || -n "${GITHUB_ACTIONS:-}" || "${LZ4RIP_SKIP_PERF:-0}" == 1 ]]; then
    echo 'Performance gate skipped (CI or LZ4RIP_SKIP_PERF=1).'
else
    cargo run --release --example perf_verify
fi
