# Changelog

## [Unreleased]

### Changed

- Faster compression when a library crate calls the encoder, as in most
  applications. There the compiler could not prove the search loop's reads
  in bounds and kept their checks, so 0.10.11 compressed up to 20% slower
  than 0.10.10. With lz4rip's bench calling it that way, against 0.10.11:
  rotating 64 B-1 MiB slices 13-47% faster, structured messages 60-81%,
  Silesia files 16% (geomean), and with a dictionary 26-31% at 64-256 B and
  -3% to +5% from 512 B. Output is byte-identical.

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
