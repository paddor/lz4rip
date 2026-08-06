# Changelog

## [Unreleased]

### Added

- JSR/WASM frame compression and decompression APIs with dictionary and
  decompressed-size-limit options.
- First-class JSR/WASM `Dictionary` object for LZ4 frame dictionary bytes and
  dictionary IDs.
- `FrameDecoderOptions` and `FrameDecoder::with_options` for configuring
  dictionaries and capping total decompressed output across concatenated LZ4
  frames.

### Changed

- Bumped the JSR/WASM package metadata to 0.2.9.

### Fixed

- The JSR package now ships `lz4rip_wasm_bg.wasm` under the filename expected
  by wasm-bindgen glue, avoiding bundler warnings for a missing WASM asset.
- `FrameDecoder` now reads concatenated LZ4 frames as one full stream,
  including empty frames before later data.

## [0.11.2] - 2026-07-20

### Added

- 32-bit Linux CI coverage for block/frame compression and decompression,
  including dictionary training and dictionary compression.
- `CompressError::InputTooLarge` for 32-bit inputs or logical compression
  streams that exceed the target's addressable safety limits.

### Changed

- 32-bit targets use the paranoid-safe encode/decode internals by default.
- On 32-bit targets, compression stream-size checks now return errors instead
  of panicking.
- Bumped the JSR/WASM package metadata to 0.2.7.

## [0.11.1] - 2026-07-13

### Changed

- Functions with caller-upheld preconditions are now `unsafe fn` in
  default builds. Each unsafe operation inside the body requires its own
  explicit `unsafe {}` block (`#![deny(unsafe_op_in_unsafe_fn)]`).
  Callers wrap each call in `paranoid_unsafe_call!(...)`, a
  cfg-conditional macro that expands to `unsafe { expr }` in default
  builds and passes through `expr` in paranoid builds.

## [0.11.0] - 2026-07-11

### Added

- `Copy` derive on `FrameInfo`.

### Changed

- The decompressor validates that the sink position does not exceed
  capacity at entry, catching corrupted state before the hot loop.
- Updated the JSR publish workflow to run `jsr publish` from GitHub
  Actions for provenance-backed CI publishing.
- Bumped JSR package `@paddor/lz4rip` to 0.2.5.

## [0.10.2] - 2026-07-07

### Changed

- `lz4rip-encode` marks the doc-hidden `compress_into_sink_with_table`
  frame-encoder plumbing as `unsafe` in default builds and documents the
  caller-owned `HashTableU32` stream invariants. The `paranoid` build keeps
  the same plumbing safe because it uses bounds-checked memory accesses.
- The main benchmark corpus now uses the standard Silesia corpus plus
  `hdfs.json`. Only `hdfs.json` remains tracked under `corpus/`; small test
  fixtures are generated in the test helper.
- Refreshed x86_64 benchmark charts from the Silesia-plus-HDFS cache and
  updated checked-in benchmark labels.
- Bumped the JSR/WASM package metadata to 0.2.4 for the `jsr-publish`
  workflow.
- Expanded JSR/WASM tests for reusable decompression, dictionary reuse,
  consumed trainers, corrupt input, and exact output-size validation. The
  `jsr-publish` workflow now runs Deno tests before publishing.

## [0.10.1] - 2026-07-05

### Added

- Frame dictionary compression now supports `BlockMode::Linked`, so blocks after the first can reference previous decoded blocks instead of only the external dictionary.

## [0.10.0] - 2026-07-04

### Added

- `paranoid` feature: compiles every crate with `#![forbid(unsafe_code)]`, replacing each unchecked memory op with a safe twin of the same signature. No `unsafe` at all; byte-for-byte compatible with the default build.
- Const-generic hash table size. `CompressorRefN`, `DictCompressorRefN`, `CompressorN`, and `DictCompressorN` take a compile-time entry count `N` (power of two, at least `MIN_ENTRIES` = 256) for memory-constrained targets, e.g. `CompressorRefN::<512>::new()` for a 2 KB table. The standard `CompressorRef`/`DictCompressorRef`/`Compressor`/`DictCompressor` are aliases at the default 8 KB size. `N` is validated at compile time.

### Changed

- Split the compressor into separate no-dict and dict types. `CompressorRef::with_dict(d)` becomes `DictCompressorRef::new(d)` and `Compressor::with_dict(d)` becomes `DictCompressor::new(d)` (breaking). `CompressorRef` no longer has a lifetime parameter. The decompressor's `with_dict` is unchanged.
- Removed the self-referential `from_raw_parts` from the owning compressor; the dictionary and hash tables are now sibling fields. `crates/encode/src/compressor.rs` is now `#![forbid(unsafe_code)]`. The isolated-unsafe boundary is now 15 blocks in 3 modules (was 16 in 4).
- Renamed crate-private fast-path helpers from `*_unchecked` to `*_inbounds` where the name describes the caller precondition rather than the default implementation.
- Paranoid encoder: remove saturating arithmetic from the safe match-length loop after validating the in-bounds precondition with debug assertions.
- Rust 2024 edition.

### Fixed

- Dictionary decompression no longer panics on corrupt input where a match spans the external dictionary and continues into the output with a remainder shorter than the match offset. `SliceSink::extend_from_within_overlapping` now clamps its overlap seed to the remainder (matching `copy_within_overlapping`), so such input returns an error instead of overshooting the output buffer. Found by `fuzz_decomp_corrupt_block`.
- The internal `HashTable` trait and generic `compress_internal` entry point are no longer re-exported. The frame encoder now uses a concrete `HashTableU32` wrapper, so downstream safe code cannot corrupt hash-table invariants that guard unchecked match reads.
- The generic `decompress_internal` entry point is no longer re-exported. The frame decoder now uses a concrete `SliceSink` wrapper, so downstream safe code cannot supply a `Sink` whose capacity does not match the output slice trusted by the unsafe fast path.

## [0.9.3] - 2026-06-28

### Changed

- Dictionary compression: read-only path for inputs <= 256 bytes skips hash table writes and uses the dict table as the sole match source.
- Dictionary compression: probe dict table before main table, restructure to keep `input_stream_offset` live in a register across both checks.
- Unchecked hash reads in `HashTableU32U16` and `HashTableU32`, eliminating bounds checks that pinned `input.len()` in a GPR. Shrinks `compress_with_dict_table` by 168 bytes.
- Drop `shrink_to_fit()` on compress return vecs.

### Fixed

- Skip slow tests (30K+ byte inputs) under miri.

## [0.9.2] - 2026-06-27

### Changed

- Exclude `jsr/` from crate package.

## [0.9.1] - 2026-06-27

### Added

- Compile-time static assertion that `Compressor` does not implement `Clone` (self-referential struct; cloning would be UB).
- 16 adversarial dictionary compression tests covering boundary-crossing matches, compressor reuse, corrupted/wrong-dict decompression, edge sizes, and the u16 table fallback. All pass under miri.

## [0.9.0] - 2026-06-24

### Security

- Bound `read_integer` loop to prevent CPU-time DoS on crafted streams with long runs of 0xFF continuation bytes.
- `get_maximum_output_size` saturates to `usize::MAX` on 32-bit overflow instead of truncating.
- Enforce `Compressor` drop order with `ManuallyDrop` and explicit `Drop` impl (previously relied on field declaration order).

### Added

- `Decompressor::new()` constructor and `Default` impl for decompression without a dictionary.
- JSR/WASM package (`@paddor/lz4rip`): LZ4 block compression compiled to WebAssembly with dictionary support.

### Fixed

- Dead code on `wasm32`: gated `get_batch_arch` and `U32_HASH_BYTES` to 64-bit targets.

## [0.8.5] - 2026-06-20

- Added `SECURITY.md` with private vulnerability reporting instructions.

## [0.8.4] - 2026-06-20

- Updated DESIGN.md and SAFETY.md for the crate split: file paths, unsafe block inventory, compile-time specialization, 32-bit hash fallback, crate split rationale section.
- Chart legend: "encapsulated unsafe" replaces "safe API".

## [0.8.3] - 2026-06-19

- Removed `doc/charts/**/*.svg` from the facade crate's `include` list. Charts use absolute GitHub URLs and were dead weight (~89 KB).
- Added `README.md` to `lz4rip-core`, `lz4rip-encode`, and `lz4rip-decode` subcrates.

## [0.8.2] - 2026-06-19

### Fixed

- **Soundness**: `VerifiedSliceSink` was `#[doc(hidden)] pub` with a safe constructor, but its `Sink` methods used `get_unchecked_mut`. Safe code could construct one and trigger OOB writes. Removed the pub re-export; the frame module now uses `SliceSink` instead.
- **Soundness**: `HashTable::get_at`/`put_at` were safe trait methods using `get_unchecked` on fixed-size arrays. `HashTableU32` was `#[doc(hidden)] pub`, so external code could call `get_at(999999)` and read OOB. Switched to checked indexing.
- **Soundness**: `HashTable::get_hash_at_unchecked` did unchecked pointer reads from safe code. Replaced per-type unsafe implementations with a default impl that delegates to the checked `get_hash_at`.
- Hardened `count_same_bytes_unchecked`: replaced `source.len() - candidate` with `saturating_sub` to prevent wrapping on precondition violation.
- Expanded safety comment on `Compressor` to document all five invariants for the self-referential `from_raw_parts`.

## [0.8.1] - 2026-06-18

### Fixed

- `AutoFinishEncoder::write`/`flush` now return `io::Error` instead of panicking when called after `finish()`.
- `FrameDecoder::read_block` now returns `io::Error` instead of panicking when no frame header has been read.
- Removed dead code: unused `MATCH_LEN_MASK` constant, stale `#[allow(dead_code)]` on `HashTableU32::reposition`.
- Fixed clippy pedantic lints: unreadable numeric literals, uninlined format args, unnecessary semicolon, `doc_markdown` backticks.

### Added

- `#[must_use]` on `compress()`, `get_maximum_output_size()`, `CompressorRef::new()`, `CompressorRef::with_dict()`.

## [0.8.0] - 2026-06-18

### Breaking

- `Decompressor::new(dict)` renamed to `Decompressor::with_dict(dict)`, restoring the v0.5 constructor name. Same rename on `DecompressorRef`.

### Changed

- Chart legend: "lz4rip (safe API, Rust)".

## [0.7.0] - 2026-06-18

### Breaking

- `Compressor<'a>` renamed to `CompressorRef<'a>` (no-alloc, borrows dictionary).
- `Decompressor<'a>` renamed to `DecompressorRef<'a>` (no-alloc, borrows dictionary).

### Added

- `Compressor` (no lifetime parameter, requires `alloc`): owns dictionary as `Vec<u8>`, restoring the v0.5 ergonomic API. Wraps `CompressorRef` internally with one contained `unsafe` for the self-referential borrow.
- `Decompressor` (no lifetime parameter, requires `alloc`): owns dictionary as `Vec<u8>`, delegates to `DecompressorRef` per call. Zero `unsafe`.

## [0.6.0] - 2026-06-18

### Breaking

- `Compressor` is now `Compressor<'a>`: borrows the dictionary (`&'a [u8]`) instead of cloning into `Vec<u8>`. `Compressor::new()` returns `Compressor<'static>`. Code that stored `Compressor` in a struct without a lifetime parameter needs updating.
- `Decompressor` is now `Decompressor<'a>`: borrows the dictionary instead of cloning. Same migration as `Compressor`.

### Added

- No-alloc support for block compress and decompress. Hash tables are stack-allocated (~8 KB) when the `alloc` feature is off. All `_into` functions, `Compressor`, and `Decompressor` work without `alloc`.
- `alloc` feature flag on `lz4rip`, `lz4rip-encode`, and `lz4rip-decode`. `std` implies `alloc`. Without `alloc`, `compress()`/`decompress()` (Vec-returning), and `DictTrainer` are unavailable.
- `compress_into_with_dict(input, output, dict)` free function for one-shot dictionary compression without a `Compressor` struct.
- CI: `cargo check --no-default-features` for encode, decode, and facade crates.

## [0.5.2] - 2026-06-17

- Fixed nightly clippy: removed duplicated `#[forbid(unsafe_code)]` attributes in `lz4rip-encode` and `lz4rip-decode` (redundant with crate-level `#[forbid]` on `mod` declarations), added `Default` impl for `HashTableU32`

## [0.5.1] - 2026-06-16

- Fixed version in README dependency example

## [0.5.0] - 2026-06-16

### Changed

- Split into workspace: `lz4rip-core`, `lz4rip-encode`, `lz4rip-decode`. Encoder and decoder are separate LLVM compilation units, eliminating the LTO poisoning class of regressions (dead encoder code shifting register allocation in the decoder). Public API unchanged.
- Decompress: `likely()` hint on fast-path gate. Previously a dead end due to register allocation interference when encoder and decoder shared a module; the crate split makes it effective. 4-17% decompress improvement across Silesia corpus, gap vs C lz4 drops from 10-33% to 2-21%.

## [0.4.0] - 2026-06-11

### Breaking

- Removed legacy frame decoding (`LZ4F_LEGACY_MAGIC_NUMBER`, `FrameInfo::is_legacy_frame()`, `BlockSize::Max8MB`). Only standard LZ4 frames (magic `0x184D2204`) are supported.

### Changed

- Hash tables are 8 KB across the board: 4K×u16 (8 KB) for inputs below 64 KB, 2K×u32 (8 KB) above. Half the 16 KB that C lz4 and lz4_flex use.
- `Compressor` uses epoch-based table reuse for inputs up to 8 KB, skipping the hash table clear between calls
- `Compressor::with_dict` uses dual 2K×u16 tables (8 KB total): a cleared main table and a read-only pristine table probed on main-table miss. Falls back to single-table path when dict+input exceeds u16 range.
- `Compressor` internals restructured as `Plain`/`Dict` enum, each variant holds only the tables it needs
- `compress_internal` is self-contained (no `Option<&T>` dict_table parameter in the hot loop). Separate `compress_with_dict_table` for the dual-table dict path.

### Added

- `DictTrainer`: COVER-based dictionary trainer for block compression. Collects samples, selects high-frequency segments, outputs a raw dict for `Compressor::with_dict`

## [0.3.1] - 2026-06-08

- Added `categories = ["compression"]` to Cargo.toml metadata

## [0.3.0] - 2026-06-07

### Breaking

- Removed `compress_prepend_size` and `decompress_size_prepended` (free functions and `Compressor`/`Decompressor` methods). The 4-byte-size-prefixed block format was non-standard. Callers who need a size prefix can prepend it themselves.
- Removed `uncompressed_size` helper from `block` module.

## [0.2.1] - 2026-06-05

- README: moved performance charts to top, added sweep chart, moved CVE table to SAFETY.md
- Bench: pipeline_chart filters inputs to displayed codecs (fixes stray json_1k.json on aarch64)
- Charts: unified widths (summary/pipeline/sweep 850px, dict2k 400px)

## [0.2.0] - 2026-06-05

- Decompress: inline wildcopy for literals (32B chunks) and matches (tiered 8/16/32B), replacing memmove calls
- Decompress: fast-path-v2 handles short-literal/long-match tokens inline, avoiding the fully bounds-checked slow path
- Decompress: unified 16B literal / 18B match copy width on both x86_64 and aarch64 (aarch64 geomean vs C lz4: 1.61x -> 1.07x)
- Decompress: removed dead `wild_copy_32` and `wild_match_copy_32` (aarch64-only, replaced by unified paths)
- Bench: added lz4_flex unsafe (v0.11, crates.io) as fourth benchmark codec
- Bench: added `--sweep` mode for input-size sweep charts (64B-1MB synthetic JSON, with/without dict)
- README: replaced compress_prepend_size example, added function overview, dictionary and frame examples

## [0.1.0] - 2026-06-05

- Originally derived from PSeitz/lz4_flex
- Remove all unsafe code paths (safe-encode/safe-decode features)
- VerifiedSliceSink: pre-verified capacity eliminates per-write bounds checks
- Hash table: 5-byte hash, 8K U16 entries, unchecked lookups
- Decompress: offset>=8 fast path with copy_nonoverlapping
- Dictionary support for blocks and frames
- forbid(unsafe_code) on compress, decompress, sink, frame modules
- Benchmark suite with pipeline chart generation
