# Design

Divergences from C lz4 and lz4_flex. Each section explains what changed, why, and the measured tradeoff.

## Skip acceleration

When the compressor fails to find a match, it skips ahead with increasing step size. The step grows by 1 every `1 << INCREASE_STEPSIZE_BITSHIFT` consecutive misses.

| | C lz4 | lz4rip |
|---|---|---|
| `INCREASE_STEPSIZE_BITSHIFT` | 6 | 3 |
| First skip at | 64 misses | 8 misses |

Measured impact (Silesia corpus, x86):
- Incompressible data (sao): 3.4x faster compression
- Literary text (dickens): 11% faster compression
- Ratio cost: ~8 percentage points on 10 MB text

A parameter sweep (`benches/param_sweep.py`) picked 3: 72 combinations of
bitshift, hash table size, and hash byte count over the full corpus. `bs3`
ranks #1 or #2 at 1 GB/s transfer.

End-to-end time of `bs3` vs `bs6` (C lz4's default) by transfer bandwidth:

| Transfer | bs3 vs bs6 | Winner |
|---|---|---|
| 1 MB/s | +2.4% | bs6 |
| 10 MB/s | +1.8% | bs6 |
| 50 MB/s | -0.1% | bs3 |
| 1 GB/s | -8.4% | bs3 |
| infinite | -10.6% | bs3 |

Below ~50 MB/s the 8 pp ratio loss dominates. Above it, bs3's compression
speed wins: memory-to-memory, IPC, local storage, datacenter networks.

All input sizes use bitshift 3. For messages up to 2 KiB, low CPU cost beats
the small size gain of a denser search. Table reuse and clearing are separate
(below).

## Hash tables

Two hash table types, distinguished by stored value width, each generic over a
compile-time entry count `N` (`crates/encode/src/hashtable.rs`):

| Table | Value width | Bytes | Standard `N` | Used by |
|---|---|---|---|---|
| `HashTableU32U16<N>` | 16-bit | `2 * N` | `DEFAULT_DICT_ENTRIES` = 4096 (8 KB) | dict path (positions fit `u16`, so dict + input < 64 KB) |
| `HashTableU32<N>` | 32-bit | `4 * N` | `DEFAULT_NODICT_ENTRIES` = 2048 (8 KB) | no-dict path, and dict path when dict + input >= 64 KB |

- **Value width** decides correctness. `u16` entries hold positions below
  64 KB, so they serve the dict path under the `dict + input < u16::MAX`
  guard. The no-dict path needs `u32`: epoch reuse moves the stream offset
  past 64 KB.
- **`N`** is a separate knob: entry count, memory (`2N`/`4N` bytes), and hash
  shift `64 - N.ilog2()`. It must be a power of two and at least
  `MIN_ENTRIES` (256, C lz4's floor), checked at compile time in `new()`.

Which table a config uses (every path honors `N`):

- **No-dict** (`CompressorRef`/`Compressor`): `HashTableU32<N>`. Inputs up to
  1 KB reuse the table: the stream offset advances, so stale entries fail the
  `MAX_DISTANCE` check. Larger inputs clear it. On a stream of different
  messages, stale entries cost 2-8 KB inputs 50-70% (text) and 10-25% (XML,
  database records) more than the clear does. Incompressible input of that
  size loses 7-26% to the clear.
- **Dict, dict + input < 64 KB** (`DictCompressorRef`/`DictCompressor`): two
  `HashTableU32U16<N>`, a main table and a read-only pristine dict table,
  probed first. Inputs up to 256 B probe only the pristine table. Inputs up to
  1 KB reuse the main table: positions start at a per-compressor base that
  advances each call, so stale entries fail the bounds check, and the table is
  cleared only when positions would pass `u16::MAX`. Larger inputs clear it.
  On a stream of different messages, reuse made 512 B and 1 KB inputs 15% and
  8% faster than the clear and gained nothing from 2 KB. Repeated 8 KB text
  inputs got 20-30% slower.
- **Dict, dict + input >= 64 KB**: positions exceed `u16`, so it builds a fresh
  `HashTableU32<N>` and runs the single-table path. A tiny-`N` dict compressor
  stays small here too (no-alloc puts this table on the stack). At `N` = 4096
  it is 16 KB, C lz4's default for large inputs.

The standard `N` gives an 8 KB L1d footprint, half the 16 KB of C lz4
(`LZ4_hash4`) and lz4_flex. Smaller `N` (e.g. `CompressorRefN::<512>`, a 2 KB
table) trades ratio for memory. Both types implement `HashTable`, so the core
loop is generic over `T` and `N`.

### Picking a table size

Measured ratio cost of shrinking the table (no-dict across the corpus, dict over
synthetic small messages):

| Workload | `N` = 256 (smallest) vs default | Notes |
|---|---|---|
| Dict, ~180 B messages | +1.1% size | only the pristine table is touched (readonly path) |
| Dict, ~700 B messages | +0.2% size | main + pristine path |
| No-dict, ≤ 1 KB inputs | ~0% | too few positions to fill more buckets |
| No-dict, 34 KB+ inputs | +20-30% size | table is the limiting factor |

Input size drives the cost, not the dict. Small messages leave the table mostly
empty (a 2 KB dict fills ~680 buckets, a short message a few hundred more), so
256 or 512 entries are nearly free and often slightly faster (less L1d
pressure). Big penalties start once inputs outgrow the table (tens of KB).

Presets for small messages: `DictCompressorRefN::<256>` (512 B/table) with a
dict, `CompressorRefN::<256>`/`<512>` (1-2 KB) without.

## 5-byte hash

On 64-bit targets, C lz4 hashes 4 bytes with a 32-bit KNUTH constant. lz4rip
hashes 5 bytes (an 8-byte native-endian load, shifted) with a 64-bit PRIME5
constant: fewer collisions in 2K-4K entry tables.

PRIME5 differs for little- and big-endian targets
(`crates/encode/src/hashtable.rs`), since the input comes from native-endian
loads.

The shift `>> (64 - N.ilog2())` folds to an immediate per `N`: no runtime cost,
no register. Standard sizes: `>> 52` for `HashTableU32U16<4096>`, `>> 53` for
`HashTableU32<2048>`.

On 32-bit targets, both tables use C lz4's 4-byte KNUTH hash.

## Compile-time specialization

Two separate `#[inline(never)]` compression hot loops in `crates/encode/src/compress.rs`:

`compress_internal` handles single-table paths (free-function API and `CompressorRef` without dict). Generic over four axes:

| Parameter | Variants | Effect |
|---|---|---|
| `T: HashTable` | `HashTableU32U16<N>`, `HashTableU32<N>` | Value width and entry count |
| `USE_DICT: bool` | true, false | Dictionary lookup code |
| `HAS_OFFSET: bool` | true, false | Offset arithmetic for dict positions |
| `S: Sink` | `SliceSink`, `VerifiedSliceSink` | Bounds-checked vs pre-verified writes |

With `USE_DICT=false`, LLVM drops all dictionary code. With `HAS_OFFSET=false`,
the offset is a constant zero.

`compress_with_dict_table` runs the dual-table dict path
(`DictCompressorRef`/`DictCompressor`), generic over `T` and `S`. It probes the
read-only pristine table first, then the main table.

`decompress_internal` (`crates/decode/src/decompress.rs`) is generic over
`USE_DICT`. It runs `decompress_loop` (`#[inline(never)]`), which decodes
through a local `OutSink` so the output slice and position stay in registers.
Two loops, like C lz4's `LZ4_FAST_DEC_LOOP`:

- **Fast loop**: tokens whose literal and match lengths need at most one
  extension byte, with offset >= 8 (or a match fully inside the dictionary),
  while both buffers have headroom. It also finishes a block that ends in a
  literal run, the common shape of small incompressible messages.
- **General path**: any other token. The fast loop leaves before consuming it.
  The general path decodes exactly that token with checked reads, then
  re-enters the fast loop.

## Forward hashing

The match-search loop in `compress_internal` hashes the next candidate while
checking the current one, hiding hash latency behind the match-check branch:

```
hash(pos+1) → check match at pos → if miss, use pre-computed hash
```

C lz4 uses the same technique. lz4_flex does not.

## Unsafe boundary

The `lz4rip` facade exposes safe compression and decompression APIs. Unsafe is
isolated to 14 blocks in five modules across three crates. Decoding itself is
safe code; the decoder's blocks only let it write into uninitialized memory.

- `crates/encode/src/hashtable.rs` (5 blocks): `count_same_bytes_inbounds`,
  `get_at`, and architecture-width batch reads. Each has `debug_assert` guards
  on bounds.
- `crates/encode/src/verified_sink.rs` (2 blocks): `VerifiedSliceSink` performs
  unchecked writes after a one-time upfront capacity check at the compression
  entry point.
- `src/frame/compress.rs` (2 blocks): `FrameEncoder` calls the doc-hidden
  `lz4rip_encode::compress_into_sink_with_table` plumbing with its private
  `HashTableU32`. The safety comments tie the table to the same linked frame
  stream, external dictionary window, and stream offset.
- `crates/decode/src/out.rs` (3 blocks): layout casts between `[u8]` and
  `[MaybeUninit<u8>]`. `out_from_init` views initialized output as decoder
  output, `out_slice` views input for copying, and `assume_init_prefix`
  views the decoded prefix as bytes.
- `crates/decode/src/decompress.rs` (2 blocks): `decompress_into_uninit`
  returns the decoded prefix via `assume_init_prefix`, and `decompress_append`
  extends the `Vec` length over it.

Default 64-bit builds decode into `[MaybeUninit<u8>]`, so callers can pass
uninitialized memory. The decoder:

- stores only bytes copied from the input, the dictionary, or output it
  already stored;
- loads output only after storing it, or from initialized history before the
  start position (each wild-copy chunk's source ends at or before its
  destination).

So every byte below the final position is initialized.
`fuzz_decomp_no_output_leak` checks that output never depends on prior buffer
contents, and Miri runs `tests/uninit.rs`. `lz4rip-decode` denies
`unsafe_code`; each site above carries `#[allow(unsafe_code)]`.

`HashTable` is crate-private. The doc-hidden `compress_into_sink_with_table`
(for `HashTableU32`) is `unsafe` in default builds: the caller's table must
belong to the same compression stream as the input.

`decompress_internal` is crate-private too. The frame decoder uses
`decompress_into_sink_with_dict` (concrete `SliceSink`), so safe code cannot
pass a `Sink` whose capacity disagrees with its slice.

`crates/encode/src/compressor.rs` is `#[forbid(unsafe_code)]`: the owning
compressors hold dictionary and tables as sibling fields, so the former
self-referential `from_raw_parts` is gone.

The decoder's copies are safe code at about unchecked cost, using the
techniques from mzstd and mbrotli:

- Fixed-size chunks (`first_chunk`, `[T; N]` loads and stores): one vector
  load and store, at most one bounds check.
- 32-bit positions (`primitives::idx`). Masking once per token tells LLVM that
  `pos + k` cannot wrap, so the fast loop's two headroom tests cover every
  chunk check. The fast loop runs only on buffers below 4 GiB.
- Windows sliced so their length is the checked quantity: an 18-byte input
  window per token, and match windows cut from their end (`tail_window`),
  whose checks equal the headroom and `offset <= pos` tests.
- Wild copies into the output headroom: 16-byte literal chunks, 8/8/2-byte
  short matches, 16-byte long-match steps, 32-byte literal chunks, and an
  8-byte repeating pattern for offsets 2 to 7.

Versus the former unchecked decoder (x86_64, median of three best-of runs):

| Workload | Change |
|---|---|
| JSON and text, 256 B-4 KiB, with or without dict | 10-26% faster |
| 64-128 B messages | 7% faster to 4% slower |
| Incompressible messages | about equal |
| Silesia corpus | 1.6% slower (0-4% per file) |

Outside the fast loop's headroom, the general path uses `.get()` with explicit
error returns.

The `paranoid` feature (see [SAFETY.md](SAFETY.md)) swaps the encoder's blocks
for safe code. The decoder runs the same code into `[u8]`. One block remains:
`out::zero_init` zero-fills an uninitialized buffer for
`decompress_into_uninit`, so `lz4rip-decode` denies `unsafe_code` instead of
forbidding it. Other crates keep `forbid(unsafe_code)`.

## Dictionary compression

`init_dict` hashes every 3rd dictionary byte, not every byte: cheaper setup,
reasonable match coverage.

`DictCompressorRef::new` / `DictCompressor::new` hash the dictionary once into
a read-only pristine `HashTableU32U16<N>`. Each `compress_into` call probes it
first, then the main table (see Hash tables for when the main table is
cleared). The dict + input < 64 KB bound makes every dict candidate a valid
offset, so the dict probe needs only a bounds check.

Dict and no-dict are separate types (`DictCompressorRef` vs `CompressorRef`,
`DictCompressor` vs `Compressor`), not a `Plain`/`Dict` enum. A no-dict
compressor carries one table, and the owning dict type needs no
self-referential `unsafe`.

Dictionaries over 64 KB (`WINDOW_SIZE`) are trimmed to their last 64 KB.
Dictionaries under `MINMATCH` (4 bytes) are ignored (use the no-dict type).

## Crate split

Four crates: `lz4rip` (facade: re-exports, frame format), `lz4rip-core`
(`Sink`, `SliceSink`, `fastcpy`, error types), `lz4rip-encode` (block
compression), `lz4rip-decode` (block decompression).

Separate encode and decode crates put them in separate LLVM codegen units.
Dead code in one path can no longer shift register allocation in the other
(1.8x LTO regression before the split). Users still depend on `lz4rip` alone.

## Scope

LZ4-HC, LZ4-OPT, and LZ4-MID (higher ratio, lower throughput) are permanently
out of scope. Use zstd ([zrip](https://github.com/paddor/zrip)) when ratio
matters.

lz4rip implements the LZ4 block format and, behind the `frame` feature, the
LZ4 frame format (streaming). The block format is the performance-critical
path.
