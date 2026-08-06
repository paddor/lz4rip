# @paddor/lz4rip

Pure Rust LZ4 block and frame codec compiled to WebAssembly. Optimized for small
messages (<8 KB) in tight loops with dictionary compression.

## Usage

```ts
import { compress, decompress, init } from "@paddor/lz4rip";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data);
const original = decompress(compressed, data.length);
```

### Frame format

Use LZ4 frames when the decoder should not need the uncompressed size:

```ts
import { compressFrame, decompressFrame, init } from "@paddor/lz4rip";

await init();

const compressed = compressFrame(data, {
  contentChecksum: true,
  contentSize: true,
});
const original = decompressFrame(compressed, {
  maxDecompressedSize: data.length,
});
```

### Reusable contexts

Amortize internal allocations across multiple compress/decompress calls:

```ts
import { Compressor, Decompressor, init } from "@paddor/lz4rip";

await init();

const compressor = new Compressor();
const c1 = compressor.compress(data1);
const c2 = compressor.compress(data2);
compressor.free();

const decompressor = new Decompressor();
const d1 = decompressor.decompress(c1, len1);
const d2 = decompressor.decompress(c2, len2);
decompressor.free();
```

### Dictionary compression

For small-message workloads (log lines, JSON records, RPC payloads) that share
common structure:

```ts
import { Compressor, Decompressor, DictTrainer, init } from "@paddor/lz4rip";

await init();

// Train a dictionary from representative samples
const trainer = new DictTrainer(4096);
for (const sample of samples) trainer.addSample(sample);
const dictBytes = trainer.train();

// Compress and decompress with the dictionary
const compressor = Compressor.withDict(dictBytes);
const compressed = compressor.compress(data);
compressor.free();

const decompressor = Decompressor.withDict(dictBytes);
const original = decompressor.decompress(compressed, data.length);
decompressor.free();
```

### Synchronous initialization

When you have pre-loaded WASM bytes (e.g. bundled or read from disk):

```ts
import { compress, initSyncFromBytes } from "@paddor/lz4rip";

const wasmBytes = Deno.readFileSync("path/to/codec.wasm");
initSyncFromBytes(wasmBytes);

const compressed = compress(data);
```

## Source

Rust source and native benchmarks:
[github.com/paddor/lz4rip](https://github.com/paddor/lz4rip)
