# Changelog

## [Unreleased]

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
