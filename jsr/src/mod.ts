/**
 * @module
 *
 * Pure Rust LZ4 block codec compiled to WebAssembly. Optimized for small
 * messages (<8 KB) in tight loops with dictionary compression.
 *
 * ```ts
 * import { init, compress, decompress } from "@paddor/lz4rip";
 *
 * await init();
 *
 * const data = new TextEncoder().encode("hello world".repeat(1000));
 * const compressed = compress(data);
 * const original = decompress(compressed, data.length);
 * ```
 *
 * Reusable contexts amortize internal allocations across calls:
 *
 * ```ts
 * import { init, Compressor, Decompressor } from "@paddor/lz4rip";
 *
 * await init();
 *
 * const compressor = new Compressor();
 * const c1 = compressor.compress(data1);
 * const c2 = compressor.compress(data2);
 * compressor.free();
 *
 * const decompressor = new Decompressor();
 * const d1 = decompressor.decompress(c1, data1.length);
 * const d2 = decompressor.decompress(c2, data2.length);
 * decompressor.free();
 * ```
 *
 * Dictionary compression for small-message workloads:
 *
 * ```ts
 * import { init, Compressor, Decompressor, DictTrainer } from "@paddor/lz4rip";
 *
 * await init();
 *
 * const trainer = new DictTrainer(4096);
 * for (const sample of samples) trainer.addSample(sample);
 * const dictBytes = trainer.train();
 *
 * const compressor = Compressor.withDict(dictBytes);
 * const compressed = compressor.compress(data);
 *
 * const decompressor = Decompressor.withDict(dictBytes);
 * const original = decompressor.decompress(compressed, data.length);
 * ```
 */

import {
  initSync,
  compress as wasmCompress,
  decompress as wasmDecompress,
  compressBound as wasmCompressBound,
  Compressor as _Compressor,
  Decompressor as _Decompressor,
  DictTrainer as _DictTrainer,
} from "./pkg/lz4rip_wasm.js";

/**
 * Reusable compression context. Amortizes internal allocations across
 * multiple compress calls. Call {@linkcode Compressor.free | .free()} when done,
 * or use `using` for automatic disposal.
 *
 * @example
 * ```ts
 * const compressor = new Compressor();
 * const c1 = compressor.compress(data1);
 * const c2 = compressor.compress(data2);
 * compressor.free();
 * ```
 */
export const Compressor: typeof _Compressor = _Compressor;
/** Type alias for {@linkcode Compressor} instances. */
export type Compressor = _Compressor;

/**
 * Reusable decompression context. Amortizes internal allocations across
 * multiple decompress calls. Call {@linkcode Decompressor.free | .free()} when done,
 * or use `using` for automatic disposal.
 *
 * @example
 * ```ts
 * const decompressor = new Decompressor();
 * const d1 = decompressor.decompress(c1, len1);
 * const d2 = decompressor.decompress(c2, len2);
 * decompressor.free();
 * ```
 */
export const Decompressor: typeof _Decompressor = _Decompressor;
/** Type alias for {@linkcode Decompressor} instances. */
export type Decompressor = _Decompressor;

/**
 * COVER dictionary trainer. Feed representative samples, then call
 * {@linkcode DictTrainer.train | .train()} to produce a dictionary for
 * use with {@linkcode Compressor.withDict} / {@linkcode Decompressor.withDict}.
 *
 * @example
 * ```ts
 * const trainer = new DictTrainer(4096);
 * for (const sample of samples) trainer.addSample(sample);
 * const dictBytes = trainer.train();
 * ```
 */
export const DictTrainer: typeof _DictTrainer = _DictTrainer;
/** Type alias for {@linkcode DictTrainer} instances. */
export type DictTrainer = _DictTrainer;

let initialized = false;

/**
 * Initialize the WASM module. Must be called before any other function.
 */
export async function init(): Promise<void> {
  if (initialized) return;

  const wasmUrl = new URL("./pkg/lz4rip_wasm_bg.wasm", import.meta.url);
  const response = await fetch(wasmUrl);
  const bytes = await response.arrayBuffer();
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

/**
 * Initialize synchronously with a pre-loaded WASM binary.
 * Use when you have already loaded the WASM bytes (e.g. via `Deno.readFileSync`
 * or `fs.readFileSync` in Node.js).
 */
export function initSyncFromBytes(bytes: BufferSource): void {
  if (initialized) return;
  initSync({ module: new WebAssembly.Module(bytes) });
  initialized = true;
}

/**
 * Compress data using LZ4 block format.
 *
 * @param input The data to compress.
 * @returns Compressed LZ4 block as a `Uint8Array`.
 *
 * @example
 * ```ts
 * const compressed = compress(data);
 * ```
 */
export function compress(input: Uint8Array): Uint8Array {
  return wasmCompress(input);
}

/**
 * Decompress LZ4 block data. The uncompressed size must be known in advance
 * (LZ4 block format does not encode it).
 *
 * @param input Compressed LZ4 block.
 * @param uncompressedSize Exact size of the original data in bytes.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid, truncated, or corrupted input, or if
 *         `uncompressedSize` does not match the actual data.
 */
export function decompress(
  input: Uint8Array,
  uncompressedSize: number,
): Uint8Array {
  return wasmDecompress(input, uncompressedSize);
}

/**
 * Upper bound on compressed size for a given input length.
 * Useful for pre-allocating output buffers.
 */
export function compressBound(inputLen: number): number {
  return wasmCompressBound(inputLen);
}
