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

export class Compressor {
  constructor();
  static withDict(dict: Uint8Array): Compressor;
  compress(input: Uint8Array): Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export class Decompressor {
  constructor();
  static withDict(dict: Uint8Array): Decompressor;
  decompress(input: Uint8Array, uncompressedSize: number): Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export class DictTrainer {
  constructor(maxDictSize: number);
  addSample(data: Uint8Array): void;
  sampleCount(): number;
  train(): Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export class Dictionary {
  constructor(bytes: Uint8Array, options: DictionaryOptions);
  readonly id: number;
  readonly bytes: Uint8Array;
  free(): void;
  [Symbol.dispose](): void;
}

export function init(): Promise<void>;
export function initSyncFromBytes(bytes: BufferSource): void;
export function compress(input: Uint8Array): Uint8Array;
export function decompress(
  input: Uint8Array,
  uncompressedSize: number,
): Uint8Array;
export function compressFrame(
  input: Uint8Array,
  options?: FrameOptions,
): Uint8Array;
export function decompressFrame(
  input: Uint8Array,
  options?: FrameDecompressOptions,
): Uint8Array;
export function compressBound(inputLen: number): number;
