# lz4rip

Fast, memory-safe LZ4 compression for Rust. On par with C lz4 throughput, with safe facade APIs over a small documented unsafe boundary.

Originally derived from [lz4_flex](https://github.com/PSeitz/lz4_flex).

## Why lz4rip

- **C lz4 speed, safe by construction.** Unsafe is isolated to a few internal modules for raw memory ops. See [SAFETY.md](SAFETY.md).
- **Optional zero-unsafe build.** The `paranoid` feature compiles every crate with `#![forbid(unsafe_code)]`, swapping each unchecked op for a safe twin. No `unsafe` at all.
- **8 KB hash tables, or smaller.** Half the L1d footprint of C lz4 and lz4_flex by default; the table size is a compile-time const generic, so constrained targets can drop to a 2 KB, 1 KB, or 512 B table.
- **Built-in dictionary training.** `DictTrainer` learns a dictionary from your data. No external tools needed.
- **Hot-loop friendly.** Epoch-based table reuse skips clearing between calls for small inputs.
- **`no_std` and no-alloc ready.** Block format works without `std` or even `alloc`. Hash tables live on the stack when `alloc` is off. Frame format requires `std`.
- **32-bit Linux.** `i686-unknown-linux-gnu` and
  `armv7-unknown-linux-gnueabihf` are tested via the safe/paranoid internals.
  Streams and frames are bounded by platform address-space limits.
- **WebAssembly.** Available as [`@paddor/lz4rip`](https://jsr.io/@paddor/lz4rip) on JSR. Block compress and decompress, dictionaries.

See [DESIGN.md](DESIGN.md) for how it all works.

## Performance

lz4rip is tuned for transfer speed, not compression ratio. It skips over
non-matching data more aggressively than C lz4 and lz4_flex, which costs about
9% ratio on compressible data (2.18x vs 2.39x) but makes compression faster,
especially on incompressible input where the skip acceleration dominates.

Charts use the full 12-file Silesia corpus.

![LZ4 Pipeline Summary](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/summary.svg)

<details>
<summary>x86_64 Silesia details (per-file, prefix sweep, dictionary)</summary>

![LZ4 Pipeline Detail](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/pipeline.svg)
![LZ4 Size Sweep](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/sweep.svg)
![LZ4 Dict 2K](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/dict2k.svg)
</details>

<details>
<summary>x86_64 Silesia prefixes (with and without dictionary)</summary>

![LZ4 Structured Dict 2K](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/structured/dict2k.svg)
![LZ4 Structured No Dict](https://raw.githubusercontent.com/paddor/lz4rip/main/doc/charts/x86_64/structured/no_dict.svg)
</details>

## Block format

```rust
use lz4rip::block::{compress, decompress_into};

let input = b"Hello people, what's up?";
let compressed = compress(input);

let mut output = vec![0u8; input.len()];
let n = decompress_into(&compressed, &mut output).unwrap();
assert_eq!(&output[..n], input);
```

The `_into` variants write into a caller-provided buffer. The plain variants
allocate and require the `alloc` feature.

### No-alloc / embedded

All `_into` functions and the `CompressorRef`/`DictCompressorRef`/`DecompressorRef`
structs work without `alloc`. Hash tables are stack-allocated (8 KB per compress
call at the standard size).

On memory-constrained targets, pick a smaller hash table via the const-generic
form. `CompressorRefN::<N>` (no-dict) and `DictCompressorRefN::<N>` (dict) take an
entry count `N` (power of two, at least `MIN_ENTRIES` = 256). `CompressorRefN::<512>`
is a 2 KB table; `::<256>` is 1 KB. Smaller tables only trade compression ratio for
memory, never correctness. The standard `CompressorRef` / `DictCompressorRef` are
aliases at the default 8 KB size.

```rust
use lz4rip::block::{CompressorRefN, get_maximum_output_size};

// 2 KB no-dict hash table instead of the default 8 KB.
let mut comp = CompressorRefN::<512>::new();
let input = b"telemetry frame payload, repeated fields, repeated fields";
let mut out = [0u8; 128];
let n = comp.compress_into(input, &mut out).unwrap();
# assert!(n > 0);
```

**Recommended presets.** Measured ratio cost of shrinking the table (no-dict
across the corpus, dict over synthetic small messages):

| Workload | Preset | Footprint | Ratio cost |
|---|---|---|---|
| Dict + small messages (≤ ~1 KB) | `DictCompressorRefN::<256>` | 512 B/table | ~1% |
| Dict, with margin | `DictCompressorRefN::<512>` | 1 KB/table | ~0.5% |
| No-dict, messages ≤ ~1 KB | `CompressorRefN::<256>` | 1 KB | ~0% |
| No-dict, general small | `CompressorRefN::<512>` | 2 KB | small |

For small messages, the workload constrained targets actually compress, table
size is nearly free: a 2 KB dictionary fills only a few hundred hash buckets, so
even a 256-entry table covers the useful matches. The larger penalties (+20-30%
ratio) only appear on 34 KB+ inputs. Smaller tables are often slightly *faster*
too (less L1d pressure, cheaper clears).

**Landscape context.** LZ4 decompression needs no hash table at all, so
decode-only on tiny chips is a solved problem (ARM published a 42-instruction
Cortex-M0 decompressor). Compression is the bottleneck: C lz4 offers
`LZ4_MEMORY_USAGE` to shrink its table (down to 1 KB at `=10`), but it is a
global `#define` that rebuilds the library. No other Rust LZ4 crate exposes a
table-size knob. The const-generic approach here lets you monomorphize different
sizes in the same binary (e.g. 512 B for telemetry, 8 KB for bulk) with zero
runtime cost.

For a build with no `unsafe` at all, add the `paranoid` feature (see
[SAFETY.md](SAFETY.md)). It composes with `no_std`, no-alloc, and the table-size
knob.

```rust
use lz4rip::block::{compress_into, decompress_into, get_maximum_output_size};

let input = b"Hello people, what's up?";
let mut compressed = [0u8; 256];
let n = compress_into(input, &mut compressed).unwrap();

let mut output = [0u8; 64];
let m = decompress_into(&compressed[..n], &mut output).unwrap();
assert_eq!(&output[..m], input);
```

One-shot dictionary compression is also available without `alloc`:

```rust
use lz4rip::block::{compress_into_with_dict, decompress_into_with_dict, get_maximum_output_size};

let dict = b"shared context bytes...";
let input = b"context bytes appear in messages";
let mut compressed = [0u8; 256];
let n = compress_into_with_dict(input, &mut compressed, dict).unwrap();

let mut output = [0u8; 64];
let m = decompress_into_with_dict(&compressed[..n], &mut output, dict).unwrap();
assert_eq!(&output[..m], input);
```

### Dictionary compression

Pre-seed the compressor and decompressor with shared context for better ratios
on small messages. `DictCompressor` clones the dictionary into owned storage.
For zero-copy, use `DictCompressorRef` / `DecompressorRef`.

```rust
use lz4rip::block::{DictCompressor, Decompressor, get_maximum_output_size};

let dict = b"shared context bytes...";
let mut comp = DictCompressor::new(dict);
let decomp = Decompressor::with_dict(dict);

let input = b"context bytes appear in messages";
let mut buf = vec![0u8; get_maximum_output_size(input.len())];
let n = comp.compress_into(input, &mut buf).unwrap();

let output = decomp.decompress(&buf[..n], input.len()).unwrap();
assert_eq!(&output[..], input);
```

### Dictionary training

Build a dictionary from sample data using the built-in COVER trainer.
Requires the `alloc` feature.

```rust
use lz4rip::block::DictTrainer;

let mut trainer = DictTrainer::new(2048); // max dict size in bytes
for sample in &samples {
    trainer.add_sample(sample);
}
let dict = trainer.train();
// Use with DictCompressor::new(&dict) / Decompressor::with_dict(&dict)
```

## Frame format

The frame format (feature `frame`, on by default) wraps block compression in the
standard LZ4 frame container with checksums, content size, and streaming support.

```rust
use lz4rip::frame::{FrameEncoder, FrameDecoder};
use std::io::{Write, Read};

// Compress
// FrameEncoder::with_dictionary(wtr, dict, dict_id) for dictionary support
let mut encoder = FrameEncoder::new(Vec::new());
encoder.write_all(b"Hello frame format!").unwrap();
let compressed = encoder.finish().unwrap();

// Decompress
let mut decoder = FrameDecoder::new(&compressed[..]);
let mut output = String::new();
decoder.read_to_string(&mut output).unwrap();
assert_eq!(output, "Hello frame format!");
```

Frame blocks are independent by default for speed. Use `BlockMode::Linked` when
ratio matters more and the stream has repeated data across block boundaries:

```rust
use lz4rip::frame::{BlockMode, FrameEncoder, FrameInfo};
use std::io::Write;

const MESSAGE_REPEAT_COUNT: usize = 4096;

let frame_info = FrameInfo::new().block_mode(BlockMode::Linked);
let mut encoder = FrameEncoder::with_frame_info(frame_info, Vec::new());
encoder
    .write_all(&b"repeated data across frame blocks".repeat(MESSAGE_REPEAT_COUNT))
    .unwrap();
let compressed = encoder.finish().unwrap();
```

Linked blocks also work with frame dictionaries:

```rust
use lz4rip::frame::{BlockMode, FrameEncoder, FrameInfo};
use std::io::Write;

const DICT_ID: u32 = u32::from_be_bytes(*b"lz4r");
const MESSAGE_REPEAT_COUNT: usize = 4096;

let dict = b"shared prefix bytes";
let frame_info = FrameInfo::new().block_mode(BlockMode::Linked);
let mut encoder =
    FrameEncoder::with_dictionary(Vec::new(), dict, DICT_ID, Some(frame_info)).unwrap();
encoder
    .write_all(&b"shared prefix bytes in the message".repeat(MESSAGE_REPEAT_COUNT))
    .unwrap();
let compressed = encoder.finish().unwrap();
```

## Safety

[SAFETY.md](SAFETY.md) documents the unsafe boundary and catalogs C lz4 memory safety bugs that Rust prevents by construction.

All codec paths are fuzz-tested (12 targets covering block and frame round-trip,
dictionary round-trip, corruption resistance, cross-validation against C lz4,
output-leak detection, bitflips, crafted tokens, and tiny hash tables) and
verified under Miri on both x86_64 and aarch64.

## Development

[DEVELOPMENT.md](DEVELOPMENT.md) covers benchmarking, fuzzing, and feature flags.
