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

export const Compressor = _Compressor;
export const Decompressor = _Decompressor;
export const DictTrainer = _DictTrainer;

const EMPTY_BYTES = new Uint8Array(0);
const MAX_U32 = 0xffff_ffff;
const MAX_WASM_USIZE = 0xffff_ffff;
const FRAME_FLAG_HAS_DICTIONARY = 1 << 0;
const FRAME_FLAG_LINKED_BLOCKS = 1 << 1;
const FRAME_FLAG_BLOCK_CHECKSUMS = 1 << 2;
const FRAME_FLAG_CONTENT_CHECKSUM = 1 << 3;
const FRAME_FLAG_CONTENT_SIZE = 1 << 4;

const dictionaryInner = new WeakMap();

function validateUint32(name, value) {
  if (!Number.isInteger(value) || value < 0 || value > MAX_U32) {
    throw new RangeError(`${name} must be an integer from 0 to 4294967295`);
  }
  return value;
}

function getDictionaryInner(dictionary) {
  const inner = dictionaryInner.get(dictionary);
  if (!inner) {
    throw new TypeError("invalid or freed Dictionary");
  }
  return inner;
}

export class Dictionary {
  constructor(bytes, options) {
    if (options === undefined) {
      throw new TypeError("Dictionary options must include id");
    }
    dictionaryInner.set(this, {
      bytes: new Uint8Array(bytes),
      id: validateUint32("id", options.id),
    });
  }

  get id() {
    return getDictionaryInner(this).id;
  }

  get bytes() {
    return new Uint8Array(getDictionaryInner(this).bytes);
  }

  free() {
    dictionaryInner.delete(this);
  }

  [Symbol.dispose]() {
    this.free();
  }
}

function frameDictionary(options) {
  const dictionary = options?.dictionary;
  if (dictionary === undefined) {
    return { bytes: EMPTY_BYTES, id: 0, enabled: false };
  }
  const inner = getDictionaryInner(dictionary);
  return { bytes: inner.bytes, id: inner.id, enabled: true };
}

function maxDecompressedSize(options) {
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
let initialization;

async function loadWasmBytes(url) {
  if (url.protocol === "file:" && globalThis.process?.versions?.node) {
    const { readFile } = await import("node:fs/promises");
    return readFile(url);
  }

  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`failed to fetch ${url}: HTTP ${response.status}`);
  }
  return response.arrayBuffer();
}

function finishInitialization(module) {
  // A synchronous caller may have initialized while the load was pending.
  if (initialized) return;
  initSync({ module });
  initialized = true;
}

export function init() {
  if (initialized) return Promise.resolve();
  if (initialization) return initialization;

  initialization = (async () => {
    const wasmUrl = new URL("./pkg/lz4rip_wasm_bg.wasm", import.meta.url);
    const bytes = await loadWasmBytes(wasmUrl);
    finishInitialization(new WebAssembly.Module(bytes));
  })().catch((error) => {
    initialization = undefined;
    throw error;
  });
  return initialization;
}

export function initSyncFromBytes(bytes) {
  if (initialized) return;
  finishInitialization(new WebAssembly.Module(bytes));
}

export function compress(input) {
  return wasmCompress(input);
}

export function decompress(input, uncompressedSize) {
  return wasmDecompress(input, uncompressedSize);
}

export function compressFrame(input, options) {
  const dict = frameDictionary(options);
  const flags = (dict.enabled ? FRAME_FLAG_HAS_DICTIONARY : 0) |
    (options?.linkedBlocks === true ? FRAME_FLAG_LINKED_BLOCKS : 0) |
    (options?.blockChecksums === true ? FRAME_FLAG_BLOCK_CHECKSUMS : 0) |
    (options?.contentChecksum === true ? FRAME_FLAG_CONTENT_CHECKSUM : 0) |
    (options?.contentSize === true ? FRAME_FLAG_CONTENT_SIZE : 0);
  return wasmCompressFrame(input, dict.bytes, dict.id, flags);
}

export function decompressFrame(input, options) {
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

export function compressBound(inputLen) {
  return wasmCompressBound(inputLen);
}
