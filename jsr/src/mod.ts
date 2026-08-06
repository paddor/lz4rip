/**
 * @module
 *
 * Pure Rust LZ4 block and frame codec compiled to WebAssembly. Optimized for
 * small messages (<8 KB) in tight loops with dictionary compression.
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
 *
 * LZ4 frame dictionaries carry a caller-assigned 32-bit dictionary ID:
 *
 * ```ts
 * import { init, compressFrame, decompressFrame, Dictionary } from "@paddor/lz4rip";
 *
 * await init();
 *
 * const dict = new Dictionary(dictBytes, { id: 0x1234 });
 * const compressed = compressFrame(data, { dictionary: dict });
 * const original = decompressFrame(compressed, {
 *   dictionary: dict,
 *   maxDecompressedSize: data.length,
 * });
 * ```
 */

import {
  compress as wasmCompress,
  compressBound as wasmCompressBound,
  compressFrame as wasmCompressFrame,
  Compressor as _Compressor,
  decompress as wasmDecompress,
  decompressFrame as wasmDecompressFrame,
  Decompressor as _Decompressor,
  DictTrainer as _DictTrainer,
  initSync,
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

export interface DictionaryOptions {
  /** 32-bit dictionary identifier stored in LZ4 frame headers. */
  id: number;
}

export interface FrameOptions {
  /** External dictionary for LZ4 frame compression. */
  dictionary?: Dictionary;
  /** Enable linked blocks so blocks can reference previous block output. */
  linkedBlocks?: boolean;
  /** Include per-block checksums. */
  blockChecksums?: boolean;
  /** Include a checksum for the full decompressed content. */
  contentChecksum?: boolean;
  /** Store the total uncompressed content size in the frame header. */
  contentSize?: boolean;
}

export interface FrameDecompressOptions {
  /** External dictionary expected by the frame stream. */
  dictionary?: Dictionary;
  /** Maximum decompressed bytes allowed across the full input stream. */
  maxDecompressedSize?: number;
}

const EMPTY_BYTES = new Uint8Array(0);
const MAX_U32 = 0xffff_ffff;
const MAX_WASM_USIZE = 0xffff_ffff;
const FRAME_FLAG_HAS_DICTIONARY = 1 << 0;
const FRAME_FLAG_LINKED_BLOCKS = 1 << 1;
const FRAME_FLAG_BLOCK_CHECKSUMS = 1 << 2;
const FRAME_FLAG_CONTENT_CHECKSUM = 1 << 3;
const FRAME_FLAG_CONTENT_SIZE = 1 << 4;

interface DictionaryInner {
  bytes: Uint8Array;
  id: number;
}

const dictionaryInner = new WeakMap<Dictionary, DictionaryInner>();

function validateUint32(name: string, value: number): number {
  if (!Number.isInteger(value) || value < 0 || value > MAX_U32) {
    throw new RangeError(`${name} must be an integer from 0 to 4294967295`);
  }
  return value;
}

function getDictionaryInner(dictionary: Dictionary): DictionaryInner {
  const inner = dictionaryInner.get(dictionary);
  if (!inner) {
    throw new TypeError("invalid or freed Dictionary");
  }
  return inner;
}

/**
 * LZ4 frame dictionary. Pairs dictionary bytes with the 32-bit dictionary ID
 * written to and checked from frame headers.
 *
 * @example
 * ```ts
 * const dict = new Dictionary(dictBytes, { id: 0x1234 });
 * const compressed = compressFrame(data, { dictionary: dict });
 * dict.free();
 * ```
 */
export class Dictionary {
  constructor(bytes: Uint8Array, options: DictionaryOptions) {
    if (options === undefined) {
      throw new TypeError("Dictionary options must include id");
    }
    dictionaryInner.set(this, {
      bytes: new Uint8Array(bytes),
      id: validateUint32("id", options.id),
    });
  }

  get id(): number {
    return getDictionaryInner(this).id;
  }

  get bytes(): Uint8Array {
    return new Uint8Array(getDictionaryInner(this).bytes);
  }

  free(): void {
    dictionaryInner.delete(this);
  }

  [Symbol.dispose](): void {
    this.free();
  }
}

function frameDictionary(
  options?: Pick<FrameOptions, "dictionary">,
): { bytes: Uint8Array; id: number; enabled: boolean } {
  const dictionary = options?.dictionary;
  if (dictionary === undefined) {
    return { bytes: EMPTY_BYTES, id: 0, enabled: false };
  }
  const inner = getDictionaryInner(dictionary);
  return { bytes: inner.bytes, id: inner.id, enabled: true };
}

function maxDecompressedSize(
  options?: FrameDecompressOptions,
): number | undefined {
  const max = options?.maxDecompressedSize;
  if (max === undefined) return undefined;
  if (!Number.isSafeInteger(max) || max < 0 || max > MAX_WASM_USIZE) {
    throw new RangeError(
      "maxDecompressedSize must be an integer from 0 to 4294967295",
    );
  }
  return max;
}

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
 * Compress data using LZ4 frame format. The frame carries metadata needed for
 * decompression, so callers do not need to know the uncompressed size.
 *
 * @param input The data to compress.
 * @param options Optional frame settings.
 * @returns Compressed LZ4 frame as a `Uint8Array`.
 */
export function compressFrame(
  input: Uint8Array,
  options?: FrameOptions,
): Uint8Array {
  const dict = frameDictionary(options);
  const flags = (dict.enabled ? FRAME_FLAG_HAS_DICTIONARY : 0) |
    (options?.linkedBlocks === true ? FRAME_FLAG_LINKED_BLOCKS : 0) |
    (options?.blockChecksums === true ? FRAME_FLAG_BLOCK_CHECKSUMS : 0) |
    (options?.contentChecksum === true ? FRAME_FLAG_CONTENT_CHECKSUM : 0) |
    (options?.contentSize === true ? FRAME_FLAG_CONTENT_SIZE : 0);
  return wasmCompressFrame(
    input,
    dict.bytes,
    dict.id,
    flags,
  );
}

/**
 * Decompress LZ4 frame data.
 *
 * @param input Compressed LZ4 frame or concatenated frames.
 * @param options Optional dictionary and output limit settings.
 * @returns Decompressed data as a `Uint8Array`.
 * @throws On invalid, truncated, or corrupted input, dictionary mismatch, or
 *         when `maxDecompressedSize` is exceeded.
 */
export function decompressFrame(
  input: Uint8Array,
  options?: FrameDecompressOptions,
): Uint8Array {
  const dict = frameDictionary(options);
  const max = maxDecompressedSize(options);
  return wasmDecompressFrame(
    input,
    dict.bytes,
    dict.id,
    dict.enabled,
    max ?? 0,
    max !== undefined,
  );
}

/**
 * Upper bound on compressed size for a given input length.
 * Useful for pre-allocating output buffers.
 */
export function compressBound(inputLen: number): number {
  return wasmCompressBound(inputLen);
}
