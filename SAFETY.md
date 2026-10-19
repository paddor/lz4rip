# Safety

## No unsafe

All four crates (`lz4rip`, `lz4rip-core`, `lz4rip-encode`, `lz4rip-decode`)
are `#![forbid(unsafe_code)]`. No `unsafe` is exposed in the public API.

The hot loops keep most bounds checks out by construction. The match finder
reads a candidate only under a test the compiler can see bounds the read, and
the hash tables are indexed by masked hashes. Compressed output goes through a
shrinking slice: one length test covers a whole short sequence. The match
counter compares two slices cut to one length. If a match finder invariant
broke, a read would hit the wrong bytes or panic, never leave the input; every
match is verified against the input bytes, so the output stays valid. The
decoder copies fixed-size chunks with one length check each. See
[DESIGN.md](DESIGN.md) for how the checks compile away.

`decompress_append` and the `Vec`-returning `decompress` zero-fill the new
bytes before decoding into them.

The `paranoid` feature has no effect today. It stays so existing feature
lists keep building, and as a guarantee: if a later release adds `unsafe`
code for speed, builds with `paranoid` leave it out.

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
