# Design

Divergences from C lz4 and lz4_flex: what changed, why, and the measured
tradeoff.

## Skip acceleration

After a failed match, the compressor skips ahead with a growing step. The step
grows by 1 every `1 << INCREASE_STEPSIZE_BITSHIFT` consecutive misses. C lz4
uses 6 (first skip after 64 misses). lz4rip uses 3 (after 8) for every input
size: for messages up to 2 KiB, low CPU cost beats the small gain of a denser
search.

Measured on Silesia (x86): 3.4x faster on incompressible data (sao), 11%
faster on text (dickens), ~8 pp worse ratio on 10 MB text. A sweep
(`benches/param_sweep.py`, 72 combinations of bitshift, table size, and hash
bytes over the corpus) ranks 3 at #1 or #2 at 1 GB/s.

End to end (compress plus transfer), 3 vs 6 takes 2.4% longer at 1 MB/s, 1.8%
at 10 MB/s, breaks even at 50 MB/s, and saves 8.4% at 1 GB/s (10.6% with
infinite bandwidth). Below ~50 MB/s the ratio loss dominates. Above it
(memory, IPC, local storage, datacenter networks) compression speed wins.

## Hash tables

Two table types in `crates/encode/src/hashtable.rs`, each generic over a
compile-time entry count `N`:

| Table | Value | Bytes | Default `N` | Used by |
|---|---|---|---|---|
| `HashTableU32U16<N>` | `u16` | `2 * N` | 4096 (8 KB) | dict path, dict + input < 64 KB |
| `HashTableU32<N>` | `u32` | `4 * N` | 2048 (8 KB) | no-dict path; dict path, dict + input >= 64 KB |

Value width decides correctness: `u16` positions fit only the dict path's
64 KB bound, and no-dict epoch reuse moves the stream offset past 64 KB. `N`
sets memory and the hash shift. It must be a power of two and at least
`MIN_ENTRIES` (256, C lz4's floor), checked at compile time. The defaults use
8 KB of L1d, half of C lz4 and lz4_flex. Table reuse:

- **No-dict**: inputs up to 1 KB (`EPOCH_THRESHOLD`) reuse the table. The
  stream offset advances, so stale entries fail the candidate test. Larger
  inputs clear it: on a stream of different messages, stale entries cost
  2-8 KB inputs 50-70% (text) and 10-25% (XML, records) more than the clear.
  Incompressible input of that size loses 7-26% to the clear.
- **Dict, dict + input < 64 KB**: a main table plus a read-only pristine dict
  table, probed first. Inputs up to 256 B probe only the pristine table.
  Inputs up to 1 KB reuse the main table from a per-compressor base that
  advances each call; it is cleared only when positions would pass
  `u16::MAX`. Reuse made 512 B and 1 KB 15% and 8% faster than clearing,
  gained nothing from 2 KB, and made repeated 8 KB text 20-30% slower, so
  larger inputs clear it.
- **Dict, dict + input >= 64 KB**: a fresh `HashTableU32<N>` seeded from the
  dict, run through the single-table path (on the stack without `alloc`).
  The dict then helps only about the first 64 KB of output.

### Picking a table size

Output size with `N` = 256 vs the default: +1.1% on ~180 B dict messages
(pristine table only), +0.2% on ~700 B dict messages, ~0% on no-dict inputs up
to 1 KB, +20-30% on no-dict inputs from 34 KB. Input size drives the cost, not
the dict: a 2 KB dict fills ~680 buckets, a
short message a few hundred more. Small tables are nearly free for small
messages and often slightly faster (less L1d pressure). Presets: with a dict
`DictCompressorRefN::<256>` (512 B/table), without `CompressorRefN::<256>` or
`<512>` (1-2 KB).

## 5-byte hash

On 64-bit targets C lz4 hashes 4 bytes with a 32-bit KNUTH constant. lz4rip
hashes 5 bytes of an 8-byte native-endian load with a 64-bit PRIME5 constant
(one per endianness): fewer collisions in 2K-4K entry tables. The shift
`>> (64 - N.ilog2())` folds to an immediate per `N`. 32-bit targets use C
lz4's 4-byte KNUTH hash.

## Compile-time specialization

Two `#[inline(never)]` hot loops in `crates/encode/src/compress.rs`.
`compress_internal` runs the single-table paths, generic over:

| Parameter | Effect |
|---|---|
| `T: HashTable` | value width and entry count |
| `USE_DICT` | dictionary lookups (dropped when false) |
| `HAS_OFFSET` | offset arithmetic for dict positions (zero when false) |
| `READONLY` | probe the pristine dict table without writing it (inputs ≤ 256 B) |
| `NEAR` | dict and input fit one 64 KiB window: no distance test |
| `S: Sink` | output sink |

`compress_with_dict_table` runs the dual-table dict path, generic over `T` and
`S`. It probes the pristine table first, then the main table.

The search loop hashes the next position while checking the current one,
hiding hash latency behind the match branch. C lz4 does the same; lz4_flex
does not.

`decompress_internal` (`crates/decode/src/decompress.rs`) is generic over
`USE_DICT` and runs `decompress_loop` (`#[inline(never)]`), which keeps the
output slice and position in registers. Two loops, like C lz4's
`LZ4_FAST_DEC_LOOP`:

- **Fast loop**: literal and match lengths with at most one extension byte,
  offset >= 8 (or a match fully inside the dict), while both buffers have
  headroom. It also finishes a block ending in a literal run, the common shape
  of small incompressible messages.
- **General path**: any other token, decoded with checked reads. The fast
  loop leaves before consuming it and resumes after.

## No unsafe

Every crate is `#![forbid(unsafe_code)]`. The hot loops keep few bounds checks
because the compiler can prove most of them.

Encoder search loops:

- Candidate reads sit under a test that bounds them. An input candidate must
  lie below `next_cur`, which just passed the loop's exit test. A dict
  candidate needs `candidate + 4 <= dict.len()`: it is a `u16`, so the add
  cannot wrap, and `init_dict` stores only positions with 8 readable bytes.
- The 4 bytes at `cur` come from `assert!(cur <= limit)` and a plain read.
  Inputs over 64 KiB have spare registers and reuse the 8 bytes read for
  hashing.
- `NEAR` inputs skip the distance test, so `input_pos < next_cur` is the only
  candidate test. Elsewhere the distance test follows the byte compare.
- On inputs over 64 KiB, the caller's dispatch gives LLVM a length range, and
  it drops the length test the check elimination needs.
  `core::hint::black_box(input)` keeps it.
- Some always-true tests stay (the epoch path's distance test, the dict loop's
  `>= main_base`). Dropping either cost 3-10% in branch misses.

Output goes through `rest`, the unwritten tail of the output slice. A short
sequence (literal run and match length below 15) takes one
`first_chunk_mut::<19>()` test and a fixed 16-byte literal copy.
`count_same_bytes` compares the first 8 bytes directly; longer matches go to
an out-of-line loop over slices cut to one length.

Versus the former unchecked encoder (x86_64, best of three, same session):

| Workload | Change |
|---|---|
| Silesia corpus | 0.7% slower |
| Rotating 2-64 KiB slices | 1.5-3% faster |
| Rotating 128 KiB-1 MiB slices | 2.7% slower |
| Rotating 512 B-1 KiB slices | 7-9% slower |
| Repeated 512 B-4 KiB messages, 2 KB dict | 5-6.5% slower |

Decoder copies:

- Fixed-size chunks (`first_chunk`, `[u8; N]` loads and stores): one vector
  load and store, at most one bounds check.
- 32-bit positions (`primitives::idx`). Masking once per token tells LLVM that
  `pos + k` cannot wrap, so the fast loop's two headroom tests cover every
  chunk check. The fast loop runs only on buffers below 4 GiB.
- Windows sliced so their length is the checked quantity: an 18-byte input
  window per token, and match windows cut from their end (`tail_window`).
- Wild copies into the output headroom: 16-byte literal chunks, 8/8/2-byte
  short matches, 16-byte long-match steps, 32-byte literal chunks, and an
  8-byte repeating pattern for offsets 2 to 7.
- Outside the headroom, the general path uses `.get()` with error returns.

Versus the former unchecked decoder (x86_64, median of three best-of runs):
JSON and text at 256 B-4 KiB, with or without dict, 10-26% faster; 64-128 B
messages 7% faster to 4% slower; incompressible messages about equal; Silesia
1.6% slower (0-4% per file).

`decompress_append` and the `Vec`-returning `decompress` zero-fill the new
bytes (`Vec::resize`), then truncate: 8-14% slower on 256 B-64 KiB blocks
than decoding into spare capacity.

`HashTable` and `decompress_internal` are crate-private. The doc-hidden
`compress_into_sink_with_table` expects a table from the same compression
stream. A foreign table reads only inside the buffers, but can produce invalid
output. The owning compressors hold dict and tables as sibling fields, so
nothing is self-referential.

## Dictionary compression

`DictCompressorRef::new` / `DictCompressor::new` hash the dict once into the
pristine table. `init_dict` hashes every 3rd byte: cheaper setup, reasonable
coverage. Each call probes the pristine table first, then the main table. The
dict + input < 64 KB bound makes every dict candidate a valid offset, so the
dict probe needs only a bounds check.

Dict and no-dict are separate types (`DictCompressorRef` vs `CompressorRef`,
`DictCompressor` vs `Compressor`), not one enum, so a no-dict compressor
carries one table. Dicts over 64 KB (`WINDOW_SIZE`) are trimmed to their last
64 KB. Dicts under 4 bytes (`MINMATCH`) are ignored.

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
