# Safety

## Unsafe boundary

The `lz4rip` facade exposes safe compression and decompression APIs. The default
build isolates unsafe code to 25 blocks in 6 modules across 3 crates. Most blocks
perform unchecked reads or writes whose bounds are proven by safe-region margins
computed in the algorithm code. The rest let the decoder write into
uninitialized memory.

The internal hash-table trait and generic decompression entry point are not
exported, so downstream safe code cannot construct invalid match candidates for
unchecked encoder reads or supply a custom `Sink` whose capacity lies to the
decoder fast path. See [DESIGN.md](DESIGN.md) for the unsafe-site inventory,
frame-compression table invariants, and safe-region margin computation.

## Paranoid build

The `paranoid` feature replaces each unchecked memory op with a safe twin
(bounds-checked indexing, `copy_within`/`copy_from_slice`, `from_ne_bytes`
reads, `chunks_exact` match counting) with the same signature, so callers are
unchanged. `lz4rip-core`, `lz4rip-encode`, and the frame module compile with
`forbid(unsafe_code)`. `lz4rip-decode` compiles with `deny(unsafe_code)` and one
exception: `decompress_into_uninit` zero-fills the caller's uninitialized buffer,
then views it as `[u8]`. Stable Rust has no safe way to make that view. The block
runs only when `decompress_into_uninit` is called. `decompress_append` uses
`Vec::resize` instead and needs no `unsafe`.

This is for users and policies that minimize `unsafe`. It trades a small amount
of throughput for the guarantee; the default build keeps the isolated-unsafe
design above. The two builds are byte-for-byte compatible: output of one
decompresses with the other.

## Why Rust matters here

C lz4's "safe" decompression API (`LZ4_decompress_safe` and variants) has had
memory safety bugs that Rust's type system and bounds checking prevent by
construction:

| CVE / Fix | Bug | Rust prevents because |
|-----------|-----|----------------------|
| [CVE-2021-3520](https://nvd.nist.gov/vuln/detail/CVE-2021-3520) (CVSS 9.8) | Negative `outputSize` wraps to huge `size_t`, heap overflow in `LZ4_decompress_safe_partial` | `usize` is unsigned, slice lengths can't be negative |
| [CVE-2014-4715](https://nvd.nist.gov/vuln/detail/CVE-2014-4715) | Integer overflow in literal-run accumulator on 32-bit causes memory corruption | Slice indexing panics on OOB regardless of overflow |
| [CVE-2022-49078](https://nvd.nist.gov/vuln/detail/CVE-2022-49078) | `LZ4_decompress_safe_partial` OOB read on corrupted data (Linux kernel's embedded copy) | Slice indexing panics on OOB |
| [PR #1753](https://github.com/lz4/lz4/pull/1753) (2026) | OOB read in `read_variable_length()`: byte read before bounds check | `.get()` checks before read |
| [PR #1733](https://github.com/lz4/lz4/pull/1733) | Negative `dictSize` cast to `size_t` bypasses offset validation in `LZ4_decompress_safe_usingDict` | `usize` can't be negative |
| [PR #1737](https://github.com/lz4/lz4/pull/1737) | `read_variable_length` overflow undetected on 64-bit (guard was `sizeof(length) < 8`) | OOB access after overflow caught by bounds check |
| [PR #1737](https://github.com/lz4/lz4/pull/1737) | Match-length overflow check misordered: `MINMATCH` added after the wraparound test | Slice index from wrapped value caught by bounds check |
| [PR #1737](https://github.com/lz4/lz4/pull/1737) | `frameRemainingSize` unsigned underflow wraps to huge value in `LZ4F_decompress` | Debug panic on underflow; OOB caught by bounds check in release |
| [e72d4423](https://github.com/lz4/lz4/commit/e72d44230093) | Fast decode loop skips dictionary bounds check, reads up to 64 KB before output buffer | `copy_within` bounded by slice length |
| [725cb0aa](https://github.com/lz4/lz4/commit/725cb0aafdf7) | `LZ4_decompress_safe_partial` reads past end of input buffer | Slice indexing panics on OOB |
| [#929](https://github.com/lz4/lz4/issues/929) | Negative input size causes memory access violation in `LZ4_decompress_safe_partial` | `usize` can't be negative |
| [#792](https://github.com/lz4/lz4/issues/792) | `read_variable_length` accumulator width is platform-dependent (`unsigned`), overflows on 16-bit | Explicit `usize` type, no platform-dependent width surprise |
| [dfc431fb](https://github.com/lz4/lz4/commit/dfc431fb3d03) | NULL pointer arithmetic UB in dictionary setup (e.g. `NULL + 0`) | No null references in Rust |
| [539c783c](https://github.com/lz4/lz4/commit/539c783c98f1) | NULL `dictBase` pointer arithmetic UB when no dictionary is provided | No null references in Rust |
