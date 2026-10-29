# @paddor/lz4rip

Pure Rust LZ4 block and frame codec compiled to WebAssembly. Optimized for small
messages (<8 KB) in tight loops with dictionary compression.

## Usage

```js
import { compress, decompress, init } from "@paddor/lz4rip";

await init();

const data = new TextEncoder().encode("hello world".repeat(1000));
const compressed = compress(data);
const original = decompress(compressed, data.length);
```

LZ4 frames carry their own metadata, so the decoder needs no size:

```js
import { compressFrame, decompressFrame } from "@paddor/lz4rip";

const framed = compressFrame(data, { contentChecksum: true });
const restored = decompressFrame(framed, { maxDecompressedSize: data.length });
```

Dictionaries help small messages that share structure:

```js
import { Compressor, Decompressor, DictTrainer } from "@paddor/lz4rip";

const trainer = new DictTrainer(4096);
for (const sample of samples) trainer.addSample(sample);
const dictBytes = trainer.train();

const compressor = Compressor.withDict(dictBytes);
const decompressor = Decompressor.withDict(dictBytes);
const roundTrip = decompressor.decompress(compressor.compress(data), data.length);
```

`initSyncFromBytes(bytes)` initializes from preloaded WASM bytes.

## Source

Rust source and native benchmarks:
[github.com/paddor/lz4rip](https://github.com/paddor/lz4rip)
