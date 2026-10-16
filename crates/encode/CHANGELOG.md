# Changelog

## [Unreleased]

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
