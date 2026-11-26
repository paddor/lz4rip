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
| `HashTableU32U16<N, H4>` | `u16` | `2 * N` | 4096 (8 KB) | dict path, dict + input < 64 KB |
| `HashTableU32<N>` | `u32` | `4 * N` | 2048 (8 KB) | no-dict path; dict path, dict + input >= 64 KB |

`H4` (4-byte hash) is only for the [ratio profile](#ratio-profile).

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
- **Dict, dict + input < 64 KB**: a main table for this input, probed first,
  plus a read-only pristine dict table. Inputs up to 128 B probe only the
  pristine table.
  Inputs up to 1 KB reuse the main table from a per-compressor base that
  advances each call; it is cleared only when positions would pass
  `u16::MAX`. Reuse made 512 B and 1 KB 15% and 8% faster than clearing,
  gained nothing from 2 KB, and made repeated 8 KB text 20-30% slower, so
  larger inputs clear it.
- **Dict, dict + input >= 64 KB**: a fresh `HashTableU32<N>` seeded from the
  dict, run through the single-table path (on the stack without `alloc`).
  The dict then helps only about the first 64 KB of output.

### Picking a table size

Output size with `N` = 256 vs the default: +0.8% on 180 B Silesia slices with
a dict, +0.2% on ~700 B dict messages, ~0% on no-dict inputs up to 1 KB,
+20-30% on no-dict inputs from 34 KB. Input size drives the cost, not the
dict: a 2 KB dict fills ~680 buckets, a
short message a few hundred more. Small tables are nearly free for small
messages and often slightly faster (less L1d pressure). Presets: with a dict
`DictCompressorRefN::<256>` (512 B/table), without `CompressorRefN::<256>` or
`<512>` (1-2 KB).

## 5-byte hash

On 64-bit targets C lz4 hashes 4 bytes with a 32-bit KNUTH constant. lz4rip
hashes the first 5 bytes in memory of an 8-byte load with a 64-bit PRIME5
constant: fewer collisions in 2K-4K entry tables. The shift
`>> (64 - N.ilog2())` folds to an immediate per `N`. The hash is `u64` math
on a little-endian view on every target, so compressed output does not
depend on pointer width or endianness.

## Compile-time specialization

Two `#[inline(never)]` hot loops in `crates/encode/src/compress.rs`.
`compress_core` runs the single-table paths, generic over:

| Parameter | Effect |
|---|---|
| `T: HashTable` | value width, entry count and hash |
| `USE_DICT` | dictionary lookups (dropped when false) |
| `HAS_OFFSET` | offset arithmetic for dict positions (zero when false) |
| `READONLY` | probe the pristine dict table without writing it (inputs ≤ 128 B) |
| `NEAR` | dict and input fit one 64 KiB window: no distance test |
| `SKIP` | skip shift: 3, or 6 for the ratio profile |
| `FILL` | index match starts and ends (ratio profile) |
| `S: Sink` | output sink |

`compress_internal` calls it with the default `SKIP` and no `FILL`, so the
default paths compile as they did before the ratio profile.

`compress_with_dict_table` runs the dual-table dict path, generic over `T` and
`S`. It probes the main table first, then the pristine table.

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
- The search loop carries the 8 bytes at the next position and compares
  their first 4 from a register. Paths with a stream offset and no dict
  (epoch reuse, linked frame blocks) and the dict loop carry only the hash.
  They read the 4 bytes at `cur` only for a candidate, at `cur.min(next_cur)`,
  which the compiler sees is bounded by the exit test.
- A test of `input.len()` alone gives the reads their length bound. Called
  from another crate, the compiler cannot see `input_pos`, and the test of
  `input.len() - input_pos` does not give it.
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
than decoding into spare capacity. Callers often read the size from an
untrusted header, so sizes from 128 KiB on take an out-of-line path that caps
them at `255 * input.len()`, the most a block can decode to. On that path an
empty `Vec` that has to grow becomes `vec![0; n]`: `alloc_zeroed` can return
untouched OS pages, so only decoded bytes cost memory. Smaller sizes pay one
compare: they have little to waste, and `calloc` skips glibc's thread cache.
That needs `#[inline(always)]` on `decompress_append_internal`. With only
`#[inline]`, the branch stopped LLVM from inlining it into callers with
several decode calls, which made small blocks up to 13% slower.

`HashTable` and `decompress_internal` are crate-private. The doc-hidden
`compress_into_sink_with_table` expects a table from the same compression
stream. A foreign table reads only inside the buffers, but can produce invalid
output. The owning compressors hold dict and tables as sibling fields, so
nothing is self-referential.

## Dictionary compression

`DictCompressorRef::new` / `DictCompressor::new` hash the dict once into the
pristine table. `init_dict` hashes every 3rd byte: cheaper setup, reasonable
coverage. The dict + input < 64 KB bound makes every dict candidate a valid
offset, so the dict probe needs only a bounds check.

Each call probes the main table (this input) first, then the pristine table,
as C lz4 does. A match within the input is usually much longer than a dict
match at the same position. Until 0.12.1 the dict came first and took short
dict hits over them. On Silesia prefixes with a trained 2 KB dict (x86_64),
input-first made output 3-12% smaller at 512 B-32 KiB, compression 3-35%
faster and decompression 6-19% faster, from fewer, longer matches. Inputs
from 64 KiB take the single-table path and did not change.

Inputs up to 128 B (`DICT_READONLY_LIMIT`) probe only the pristine table,
so they find no matches within themselves. The cutoff was 256 B; lowering
it lets 129-256 B inputs use the main table and retain those matches.

This trades speed for ratio on purpose. Compressing at 1.5 GB/s is not
needed, and at 256 B the dict path wrote larger blocks than no-dict. On
129-256 B Silesia slices with a 2 KB dict, output is 12-16% smaller on
compressible files and 7-15% on incompressible ones, for 29-70% and 27-45%
more compress + decompress time. At 256 B on compressible files compression
drops from 1525 to 895 MB/s, still 2.3x C lz4, and output goes from 14%
larger to 4% smaller than no-dict. Messages that match mostly in the dict
gain under 1% and take 10-21% more time. See the
[Silesia sweep](doc/charts/x86_64/sweep.svg).

Dict and no-dict are separate types (`DictCompressorRef` vs `CompressorRef`,
`DictCompressor` vs `Compressor`), not one enum, so a no-dict compressor
carries one table. Dicts over 64 KB (`WINDOW_SIZE`) are trimmed to their last
64 KB. Dicts under 4 bytes (`MINMATCH`) are ignored.

## Ratio profile

`compress_ratio_into`, `compress_ratio`, `RatioCompressor` and
`FrameEncoder::ratio_profile` trade compression speed for smaller output.
They are for formats that must stay LZ4, such as fixed wire formats. Where
the format is free, zstd compresses better. They run the same loop as the
default (`compress_core`) with other const parameters:

| Input (with dict) | Table | Hash | `SKIP` | `FILL` |
|---|---|---|---:|---|
| up to 4 KiB | 4K x `u16` (8 KB) | 4 bytes | 6 | no |
| 4-64 KiB | 16K x `u16` (32 KB) | 5 bytes | 6 | yes |
| from 64 KiB, other frame blocks | 16K x `u32` (64 KB) | 5 bytes | 6 | yes |

`RatioCompressor` keeps one `u32` table for both larger rows, with the same
output. Up to 256 B (`RATIO_EPOCH_THRESHOLD`) it reuses its small table
across calls instead of clearing 8 KB each time: each call stores positions
above the last call's, so older entries fail the `input_pos < next_cur`
test, and the table is cleared only when `u16` positions would wrap. The
one-shot functions store positions from 1 in a fresh table, so both write the
same output. On 4 MiB of Silesia in chunks, reuse made `RatioCompressor` 2x
as fast at 64 B and 10% faster at 256 B. Reuse runs a slower loop (stream
offset): it broke even at 512 B and was 12% slower at 1 KiB and 25% slower
at 4 KiB. Up to 4 KiB these are C lz4's settings. `FILL` indexes two
positions after each match start and two before its end, as C lz4's MID mode
does (see [below](#c-lz4s-hc-levels-and-mid)). The search stays single pass,
one probe per position.

Measured on x86_64 (i7-8700B, one core, best of 3 to 5), each chunk
compressed alone. Output % / compression MB/s:

| | Silesia 512 KiB | Markdown 512 KiB | Rust 512 KiB | mixed 4 KiB | mixed 8 KiB | mixed 64 KiB |
|---|---:|---:|---:|---:|---:|---:|
| default | 51.6 / 580 | 48.9 / 454 | 35.5 / 524 | 64.5 / 602 | 59.5 / 563 | 52.9 / 530 |
| lz4_flex 0.14.2 | 47.9 / 508 | 44.1 / 417 | 33.6 / 512 | 60.4 / 455 | 56.5 / 448 | 49.1 / 492 |
| C lz4 1.10, accel 1 | 47.8 / 509 | 44.1 / 429 | 33.5 / 516 | 60.1 / 468 | 56.2 / 461 | 48.6 / 450 |
| ratio profile | 44.5 / 377 | 40.5 / 321 | 32.2 / 434 | 60.3 / 448 | 56.3 / 489 | 46.9 / 422 |
| C LZ4-MID (HC level 2) | 42.3 / 234 | 38.2 / 224 | 29.9 / 313 | 58.3 / 255 | 53.9 / 230 | 44.6 / 209 |

Markdown and Rust are 16 MiB each of README/CHANGELOG files and source
files from crates.io crates. Mixed is 4 MiB from the start of samba,
dickens, xml and mozilla, plus the Markdown set.

Ratio output decodes 4-12% slower than default output from 8 KiB and 15-22%
slower at 1-4 KiB (more matches, and 4-byte ones). That is within 7% of C
lz4's output.

Tried and dropped (512 KiB chunks unless noted):

- 4-byte hash above 4 KiB: larger output, compression 13% slower, decoding
  17-20% slower (more 4-byte matches).
- 32K and 64K entries: 0.2-0.6 pp smaller output per doubling, 5-11% slower
  each.
- One-step lazy matching (try `cur + 1`, keep the longer match): the gain and
  cost of one table doubling.
- Skip shift 4 or 5: 0.1-0.3 pp larger output for 1-4% speed. At 8 KiB, 4
  gave 0.7 pp larger output.
- The 16K table from 4 KiB down: at 4 KiB, 0.4 pp larger output than C lz4's
  settings, and slower. At 8 KiB it wins both.
- `FILL` skipping end positions inside short matches: 7% slower, from
  branches on the match length.

### C lz4's HC levels and MID

C lz4's HC API (`LZ4_compress_HC`) maps its level to one of three searches
(lz4 1.10, `k_clTable` in `lz4hc.c`; the header documents only the range):

| Level | Search | Per position |
|---|---|---|
| 1-2 | MID | one probe in each of two tables |
| 3-9 | hash chain | up to 4, 8, 16 ... 256 earlier positions |
| 10-12 | optimal parser | up to 96, 512, 16384 candidates, then sequences picked by cost |

Levels below 1 mean 9. MID came with lz4 1.10 (2024). Before, levels 1-2
ran a two-try hash chain and the lowest real HC level was 3. 1.10 lowered
`LZ4HC_CLEVEL_MIN` to 2, so `lz4 -2` and frame level 2 run MID. Level 1
has the same parameters.

MID (`LZ4MID_compress`) is a single pass:

- Two 16K-entry `u32` tables (128 KB), one keyed by a hash of 7 bytes (long
  matches), one by a hash of 4 bytes. Each position probes the long table
  first, then the short one.
- On a short match it also probes the long table at `ip + 1` and keeps the
  longer match.
- After each match it indexes positions 1 and 2 after the start (long
  table) and 1 (short), and 5, 3 and 2 before the end (long) and 2 and 1
  (short).
- On misses the step grows by 1 per 512 bytes since the last match. Fast
  mode and the ratio profile grow it every 64 misses, which is why MID is
  slow on incompressible input.

The ratio charts and the table above compare against level 2 because it is
MID: the fastest HC level, and the same kind of search as the ratio profile
(single pass, no chains, match ends indexed). Faster than level 2 means
faster than every HC level. Levels 3 and up compress smaller and slower.
The ratio profile differs in one table with a 5-byte hash (64 KB, 8 KB up
to 4 KiB of input), no `ip + 1` probe, and the fast-mode skip.

## Frames

The frame code wraps the block code. A frame's first independent block under
64 KiB compresses as the block API does, with a fresh table sized for the
input, as `compress` and `compress_ratio` make. Later blocks, linked blocks
and blocks from 64 KiB share one table, made by the first of them
(`HashTableU32`, or 16K x `u32` for the ratio profile). It is not cleared
between blocks: positions keep growing, so entries from earlier independent
blocks fail the range test. A frame of one small block, as one frame per
message or record writes, so makes no table of its own. Before, each new
encoder made an 8 KB table, or 64 KB with the ratio profile, and compressed
through it more slowly (`u32` positions, no `NEAR`).

The decoder zeroes its output buffer up to each block's decode limit, since
safe code decodes into initialized memory only. The buffer keeps its length,
so a reused decoder zeroes each byte once. With a declared content size, the
limit is what the frame still declares. Without one, it was the block
maximum: a new decoder zeroed 64 KiB (up to 4 MiB) for a 1 KiB frame, which
took longer than decoding it. The first limit is 8x the block's compressed
length (`FIRST_LIMIT_EXPANSION`), or the bytes already zeroed if more. A
block that decodes to more is decoded again with the full limit, so errors
match an unlimited decode.

Measured on x86_64 (i7-8700B, one core, best of 10) with a new encoder or
decoder per frame, 64 slices per Silesia file, geomean change from 0.12.7:

| Frame | 256 B | 1 KiB | 4 KiB | 16 KiB | 64 KiB |
|---|---:|---:|---:|---:|---:|
| decode | -68% | -63% | -31% | -2.5% | +1.0% |
| compress, default | -11% | -9% | -10% | -6% | 0% |
| compress, ratio profile | -62% | -36% | -13% | -18% | 0% |

At 64 KiB, xml and nci decoded 8% and 4% slower: their slices that compress
more than 8x decode twice. A 4x first limit missed often enough to make
16 KiB xml and nci slices 30-40% slower. Default output got 0-1.8% smaller
(the block API's 4K-entry table below 64 KiB). Ratio output at 4 KiB follows
the block API's C lz4 settings: 0.3% larger geomean, up to 5% on reymont and
nci. Whole-file streams (64 KiB blocks, independent and linked) and frames
of flushed 200 B messages did not change.

## Crate split

Four crates: `lz4rip` (facade: re-exports, frame format), `lz4rip-core`
(`Sink`, `SliceSink`, `fastcpy`, error types), `lz4rip-encode` (block
compression), `lz4rip-decode` (block decompression).

Separate encode and decode crates put them in separate LLVM codegen units.
Dead code in one path can no longer shift register allocation in the other
(1.8x LTO regression before the split). Users still depend on `lz4rip` alone.

## Scope

LZ4-HC, LZ4-OPT, and LZ4-MID (higher ratio, lower throughput) are permanently
out of scope. The [ratio profile](#ratio-profile) stays a single-pass search
with one table. Use zstd ([zrip](https://github.com/paddor/zrip)) when ratio
matters and the format is free.

lz4rip implements the LZ4 block format and, behind the `frame` feature, the
LZ4 frame format (streaming). The block format is the performance-critical
path.
