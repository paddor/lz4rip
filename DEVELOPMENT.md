# Development

## Test suite

Run `scripts/test-all.sh` for formatting, Clippy, correctness tests, and
feature checks, in that order.

## Benchmarks

Set the CPU governor to `performance` before benchmarking to prevent frequency scaling from skewing results:

```sh
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
```

Pin to a single core with `taskset -c 0` to avoid cross-core migration noise.

All chart commands require `LZ4RIP_HW_EXTRAS` to inject the governor/turbo subtitle
on systems where sysfs is unavailable (or when the governor was not changed before
running). Always set it:

```sh
export LZ4RIP_HW_EXTRAS="performance governor,turbo off"
```

The bench downloads the 12-file Silesia corpus into ignored `corpus/silesia/`
on first run. It writes results to `~/.cache/lz4rip/<arch>/` (and subdirs for
sweep/structured). Chart generation reads from cache. Two separate steps, no
piping.

Bench all impls and generate all charts:
```sh
taskset -c 0 cargo run --release --example lz4rip_bench
taskset -c 0 cargo run --release --example lz4rip_bench -- --dict-silesia
taskset -c 0 cargo run --release --example lz4rip_bench -- --structured
taskset -c 0 cargo run --release --example lz4rip_bench -- --structured-dict
taskset -c 0 cargo run --release --example lz4rip_bench -- --sweep
taskset -c 0 cargo run --release --example lz4rip_bench -- --small
cargo run --manifest-path bench/Cargo.toml --bin lz4rip_charts -- all doc/charts/x86_64
```

`--small` benchmarks slices (512 B to 1 MiB) of four Silesia files with
reused compressor state and writes `small_encode.svg` and `small_decode.svg`.
Each timed pass walks up to 64 distinct slices of one size, so no codec sees
the same input twice in a row. Repeating one input lets the branch predictor
learn it and distorts results. Every implementation decodes C lz4's blocks
(thick lines) and its own output (thin lines).

Rerun only lz4rip (other impls served from cache), then regenerate charts:
```sh
taskset -c 0 cargo run --release --example lz4rip_bench -- --impl lz4rip
cargo run --manifest-path bench/Cargo.toml --bin lz4rip_charts -- all doc/charts/x86_64
```

Clear a specific impl's cache to force re-bench:
```sh
rm ~/.cache/lz4rip/x86_64/lz4rip.jsonl
```

Optional local hardware labels can live in ignored `.chart_hw`:

```text
prefix=Linux VM on a 2018 Mac Mini
postfix=performance governor, turbo off
```

The chart tool reads `.chart_hw` from the current dir or parent dir. Env vars
`LZ4RIP_HW_PREFIX`, `LZ4RIP_HW_POSTFIX`, and `LZ4RIP_HW_EXTRAS` override or
extend local detection.

## Releasing

`release-plz` runs on every push to `main`
(`.github/workflows/release-plz.yml`). It opens or updates a release PR,
creates annotated tags after merge, publishes to crates.io, and creates
GitHub releases. Configuration lives in `release-plz.toml`.

Publishing uses crates.io trusted publishing through GitHub Actions OIDC.
Do not add a crates.io token secret.

### Steps

1. **Review the release-plz PR.** Verify semver bumps and dependency versions
   for every affected crate.

2. **Curate changelogs.** Insert a new `## [x.y.z]` section immediately below
   `## [Unreleased]` for each bumped crate and place its release notes there.
   Never rename or modify an existing versioned section.

3. **Bump the WASM package.** Update `jsr/deno.json` and
   `jsr/wasm/Cargo.toml`.

4. **Build and verify.** Run the JSR commands below and any additional release
   audit warranted by the changes.

5. **Merge the release PR after CI passes.** Monitor `release-plz` for crate
   publication and `.github/workflows/jsr.yml` for JSR publication.
   JSR rejects duplicate versions.

6. **Confirm publication.** Verify every intended crate version on crates.io
   and the new package version on each JavaScript registry. Run roundtrips
   against the published packages, including a bundled JSR roundtrip.

```bash
cd jsr
bash build.sh
deno task test
```

Use Deno 2.8.3 or newer for the bundled tests. They cover standalone bundles,
synchronous bytes, and initialization races. Bundled default initialization
runs without file or network permissions.

## Fuzzing

Requires nightly:
```sh
cargo +nightly fuzz run fuzz_roundtrip
cargo +nightly fuzz run fuzz_roundtrip_frame
cargo +nightly fuzz run fuzz_decomp_corrupt_block
cargo +nightly fuzz run fuzz_decomp_corrupt_frame
cargo +nightly fuzz run fuzz_decomp_no_output_leak
cargo +nightly fuzz run fuzz_roundtrip_cpp_compress
cargo +nightly fuzz run fuzz_validate_block
cargo +nightly fuzz run fuzz_validate_block_dict
cargo +nightly fuzz run fuzz_decoder_reuse
```

## Feature flags

- `frame`: LZ4 frame format support. Implies `std`. Enabled by default.
- `std`: standard library dependency. Enabled by default.
- `nightly`: enables nightly-only `#[optimize(size)]` on cold paths. Not required for correctness or performance on stable.
