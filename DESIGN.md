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

The value 3 was selected by a systematic parameter sweep (`benches/param_sweep.py`) over 72 combinations of bitshift, hash table sizes, and hash byte counts across the full corpus. `bs3` ranks #1 or #2 at 1 GB/s transfer.

Crossover analysis against `bs6` (C lz4's default) at various transfer bandwidths:

| Transfer | bs3 vs bs6 | Winner |
|---|---|---|
| 1 MB/s | +2.4% | bs6 |
| 10 MB/s | +1.8% | bs6 |
| 50 MB/s | -0.1% | bs3 |
| 1 GB/s | -8.4% | bs3 |
| infinite | -10.6% | bs3 |

Below ~50 MB/s, the 8pp ratio difference dominates and bs6 is 1-2% faster
end-to-end. Above that, bs3's compression speed advantage takes over. For
memory-to-memory, IPC, local storage, and datacenter networking, bs3 is the
right choice.

**Why not raise it (bs4, bs5, bs6)?** Tested repeatedly. Ratio barely improves,
compression gets slower across the board, and incompressible data gets hit
hardest because the compressor wastes cycles hashing bytes that will never
match. LZ4 is a speed codec. Chasing ratio turns it into a worse zstd instead
of a better LZ4. The value 3 is final.

## Hash tables

Two hash table types, distinguished by stored value width, each generic over a
compile-time entry count `N` (`crates/encode/src/hashtable.rs`):

| Table | Value width | Bytes | Standard `N` | Used by |
|---|---|---|---|---|
| `HashTableU32U16<N>` | 16-bit | `2 * N` | `DEFAULT_DICT_ENTRIES` = 4096 (8 KB) | dict path (positions fit `u16`, so dict + input < 64 KB) |
| `HashTableU32<N>` | 32-bit | `4 * N` | `DEFAULT_NODICT_ENTRIES` = 2048 (8 KB) | no-dict path, and dict path when dict + input >= 64 KB |

The **value width** (`u16` vs `u32`) decides which type is correct: `u16` entries
can only store positions below 64 KB, so they serve the dict path where the
`dict + input < u16::MAX` guard holds; the no-dict path uses `u32` entries because
epoch reuse advances a stream offset past 64 KB. **`N` is an independent knob**: it
sets the entry count, the memory (`2N`/`4N` bytes), and the hash shift
`64 - N.ilog2()`. `N` must be a power of two and at least `MIN_ENTRIES` (256, an 8-bit index,
matching C lz4's floor), checked at compile time in `new()`.

Which table a given config uses:

- **No-dict** (`CompressorRef`/`Compressor`): always `HashTableU32<N>`. `N` is honored for every call. Epoch-based reuse for inputs up to 8 KB advances a stream offset instead of clearing, so stale entries fall outside `MAX_DISTANCE` and are rejected by the distance check.
- **Dict** (`DictCompressorRef`/`DictCompressor`) with dict + input < 64 KB: two `HashTableU32U16<N>` tables, a cleared main table and a read-only pristine table probed on main-table miss. `N` is honored.
- **Dict** with dict + input >= 64 KB: positions exceed `u16`, so it builds a fresh `HashTableU32<N>` sized to the compressor's own `N` and runs the single-table path. `N` is honored here too: a tiny-`N` dict compressor stays small even on the overflow path (relevant for no-alloc, where that table is a stack frame). At the standard `N` (4096) this is a 16 KB `u32` table, matching C lz4's default table size for large inputs.

The standard `N` gives a consistent 8 KB L1d footprint, half the 16 KB that C lz4
(`LZ4_hash4`) and lz4_flex use. Smaller `N` (e.g. `CompressorRefN::<512>` for a
2 KB table) trades ratio for memory on constrained targets. Both types implement
the `HashTable` trait so the core loop is generic over `T` (and thus over `N`).

### Picking a table size

Measured ratio cost of shrinking the table (no-dict across the corpus, dict over
synthetic small messages):

| Workload | `N` = 256 (smallest) vs default | Notes |
|---|---|---|
| Dict, ~180 B messages | +1.1% size | only the pristine table is touched (readonly path) |
| Dict, ~700 B messages | +0.2% size | main + pristine path |
| No-dict, ≤ 1 KB inputs | ~0% | too few positions to fill more buckets |
| No-dict, 34 KB+ inputs | +20-30% size | table is the limiting factor |

The cost is driven by input size, not whether a dict is used. For the small
messages that constrained targets actually compress, the table is far from full
(a 2 KB dict hashes to only ~680 buckets, a short message adds a few hundred more),
so dropping to a 256- or 512-entry table is nearly free, and often slightly faster
from reduced L1d pressure. The large penalties appear only once inputs exceed the
table's capacity (tens of KB). Recommended presets: `DictCompressorRefN::<256>`
(512 B/table) for dict + small messages, `CompressorRefN::<256>`/`<512>` (1-2 KB)
for no-dict small messages.

## 5-byte hash

On 64-bit targets, C lz4 hashes 4 input bytes with a 32-bit KNUTH multiplicative constant. lz4rip reads 5 bytes (via an 8-byte native-endian load, shifted) and hashes with a 64-bit PRIME5 constant. The extra byte reduces collisions across 2K-4K entry tables.

The PRIME5 constant is endianness-aware: different values for little-endian and big-endian targets in `crates/encode/src/hashtable.rs`, since the hash input comes from native-endian reads.

Hash shifts are derived from the const-generic entry count: `>> (64 - N.ilog2())`, which folds to an immediate at monomorphization (no runtime cost, no register holding the size). At the standard sizes `HashTableU32U16<4096>` uses `>> 52` and `HashTableU32<2048>` uses `>> 53`.

On 32-bit targets, both hash tables fall back to a 4-byte KNUTH multiplicative hash, matching C lz4's approach.

## Compile-time specialization

Two separate `#[inline(never)]` compression hot loops in `crates/encode/src/compress.rs`:

`compress_internal` handles single-table paths (free-function API and `CompressorRef` without dict). Generic over four axes:

| Parameter | Variants | Effect |
|---|---|---|
| `T: HashTable` | `HashTableU32U16<N>`, `HashTableU32<N>` | Value width and entry count |
| `USE_DICT: bool` | true, false | Dictionary lookup code |
| `HAS_OFFSET: bool` | true, false | Offset arithmetic for dict positions |
| `S: Sink` | `SliceSink`, `VerifiedSliceSink` | Bounds-checked vs pre-verified writes |

When `USE_DICT=false`, all dictionary code is dead and eliminated by LLVM. When `HAS_OFFSET=false`, offset is a compile-time zero. LLVM specializes each call site independently without excessive code duplication.

`compress_with_dict_table` handles the dual-table dict path (`DictCompressorRef`/`DictCompressor`). Generic over `T: HashTable` and `S: Sink`. Takes both a cleared main table and a read-only pristine table, probing the pristine table on main-table miss.

`decompress_internal` in `crates/decode/src/decompress.rs` is generic over `USE_DICT: bool` and `S: Sink` (only `SliceSink` in practice). Fast path: unchecked reads via `primitives.rs` in the safe region. Slow path: bounds-checked near buffer end.

## Forward hashing

The match-search loop in `compress_internal` computes the hash of the next candidate position while checking the current one. This hides hash computation latency behind the branch misprediction penalty of the match check. The pattern is:

```
hash(pos+1) → check match at pos → if miss, use pre-computed hash
```

C lz4 uses the same technique. lz4_flex does not.

## Pure safe Rust

Every crate in the workspace has `#![forbid(unsafe_code)]`. All memory
operations use bounds-checked indexing, `copy_from_slice`, `copy_within`, and
`from_ne_bytes`. There is no `unsafe` anywhere in the library.

`decompress_internal` is crate-private. The facade crate's frame decoder uses a
concrete `decompress_into_sink_with_dict` wrapper for `SliceSink`, so external
code cannot supply a custom `Sink` whose reported capacity diverges from the
output slice.

The safe-region margin computation in `decompress_internal` determines how far
from buffer ends the fast path can operate. Inside the margin, wild copies in
`primitives.rs` use fixed-size `copy_from_slice` / `copy_within`. Outside it,
the slow path uses `.get()` with explicit error returns.

## Dictionary compression

Dictionary initialization in `init_dict` hashes every 3rd byte of the dictionary, not every byte. This reduces setup cost while maintaining reasonable match coverage.

`DictCompressorRef::new` / `DictCompressor::new` hash the dictionary once into a read-only pristine `HashTableU32U16<N>`. Each `compress_into` call clears the main table and probes the pristine table on miss. Dict and no-dict are separate types (`DictCompressorRef` vs `CompressorRef`, `DictCompressor` vs `Compressor`) rather than a `Plain`/`Dict` enum, so a no-dict compressor carries only its single table and the owning dict type needs no self-referential `unsafe`.

Dictionaries larger than 64 KB (`WINDOW_SIZE`) are trimmed to the last 64 KB; dictionaries shorter than `MINMATCH` (4 bytes) are ignored (use the no-dict type).

## Crate split

The workspace has four crates: `lz4rip` (facade, re-exports + frame format), `lz4rip-core` (shared types: `Sink`, `SliceSink`, `fastcpy`, error types), `lz4rip-encode` (block compression), `lz4rip-decode` (block decompression).

Encoding and decoding are in separate crates so LLVM compiles them in separate codegen units. This eliminates a class of LTO-induced regressions where dead code in one path shifts register allocation in the other, causing measurable throughput changes (1.8x observed before the split). The facade crate re-exports the public API so downstream users see a single `lz4rip` dependency.

## Scope

LZ4-HC, LZ4-OPT, and LZ4-MID are permanently out of scope. These are higher-ratio, lower-throughput compression modes. Use zstd ([zrip](https://github.com/paddor/zrip)) when ratio matters.

lz4rip implements LZ4 block format and LZ4 frame format (streaming, behind `frame` feature flag). The block format is the performance-critical path.
