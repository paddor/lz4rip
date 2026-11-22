# Changelog

## [Unreleased]

## [0.10.15] - 2026-11-22

### Changed

- Dictionary compression finds matches within 129-256 B inputs too
  (`DICT_READONLY_LIMIT` is now 128 B, was 256 B). With a 2 KB dict, output
  of 129-256 B Silesia slices is 12-16% smaller on compressible files and
  7-15% on incompressible ones. At 256 B it is now smaller than no-dict
  output.
  This trades speed for ratio on purpose: compress + decompress takes
  29-70% and 27-45% longer. At 256 B on compressible files, compression
  drops from 1525 to 895 MB/s, still 2.3x C lz4 (x86_64). Messages that
  match mostly in the dict gain under 1% and take 10-21% longer.
  Output differs from 0.10.14 for these sizes. Other sizes and no-dict
  compression are unchanged.

## [0.10.14] - 2026-11-18

### Changed

- Compressed output is the same on every target. 32-bit targets (including
  wasm32) and big-endian targets now hash like 64-bit little-endian ones, so
  their output differs from earlier versions. wasm32 compresses 32% faster
  (geomean, Silesia), i686 8% faster. x86_64 is unchanged.
  Big-endian targets hashed the wrong bytes before and found fewer matches:
  on s390x, a 4 KiB text sample compressed 36% larger.

## [0.10.13] - 2026-11-14

### Added

- `Debug` for `DictTrainer`. It prints sizes, not sample contents.

### Changed

- Dictionary compression looks for matches within the input before matches
  in the dict, as C lz4 does. On 512 B-32 KiB inputs output is 3-12%
  smaller, compression 3-35% faster and decompression 6-19% faster (Silesia
  prefixes, trained 2 KB dict, x86_64). Inputs up to 256 B and from 64 KiB
  are unchanged. Output differs from 0.10.12.

## [0.10.12] - 2026-10-20

### Changed

- Faster compression when a library crate calls the encoder, as in most
  applications. There the compiler could not prove the search loop's reads
  in bounds and kept their checks, so 0.10.11 compressed up to 20% slower
  than 0.10.10. With lz4rip's bench calling it that way, against 0.10.11
  (geomean per size): 64 B-8 KiB 10-26% faster, 16-32 KiB 13%, 64 KiB-1 MiB
  2-3%, structured messages 36%, whole Silesia files 2%. With a dictionary:
  18-23% at 64-256 B, flat (±1%) at 512 B-32 KiB, 4-7% from 64 KiB. Some
  very compressible inputs are slower: `nci` 4-5% from 128 KiB, single
  16-64 KiB slices of `nci`, `xml` and `reymont` 6-17%. Output is
  byte-identical.

## [0.10.11] - 2026-10-20

### Changed

- No `unsafe` code: the crate is `#![forbid(unsafe_code)]`. Reads, the match
  counter, and output writes are safe code, and the output is byte-identical.
  Compression time against 0.10.10: Silesia corpus 0.7% slower, rotating
  2-64 KiB slices 1.5-3% faster, 128 KiB-1 MiB 2.7% slower, 512 B-1 KiB 7-9%
  slower, repeated 512 B-4 KiB messages with a dictionary 5-6.5% slower.
- The doc-hidden `compress_into_sink_with_table` is a safe `fn`.
- The `paranoid` feature has no effect.
- MSRV is 1.89.

## [0.10.10] - 2026-10-19

### Changed

- Dict compression is faster on small inputs, with byte-identical output:
  35-40% at 128-256 B, 20% at 512 B, 12% at 1 KB, and 3% at 2-16 KB on a
  stream of different messages. `DictCompressor` and `DictCompressorRef` reuse
  their main table across calls for inputs up to 1 KB instead of clearing it,
  and the dict probe skips checks the dict + input < 64 KB bound already
  covers.

## [0.10.9] - 2026-10-17

### Changed

- Depend on `lz4rip-core` `0.6.0`.

### Fixed

- `--features nightly` builds again on nightly 1.101 and newer, which
  stabilized `optimize_attribute`.

## [0.10.8] - 2026-09-27

### Fixed

- Reduce compression CPU cost for small blocks by restoring faster match
  skipping, at the cost of slightly larger output.
