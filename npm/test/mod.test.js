import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import test from "node:test";
import {
  compress,
  compressBound,
  compressFrame,
  Compressor,
  decompress,
  decompressFrame,
  Decompressor,
  Dictionary,
  DictTrainer,
  init,
  initSyncFromBytes,
} from "../src/mod.js";

await init();

const encode = (text) => new TextEncoder().encode(text);

function concat(...parts) {
  const output = new Uint8Array(parts.reduce((n, part) => n + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.length;
  }
  return output;
}

test("one-shot round-trip", () => {
  const data = encode("hello world, hello lz4!".repeat(100));
  const compressed = compress(data);
  assert.ok(compressed.length < data.length);
  assert.deepEqual(decompress(compressed, data.length), data);
});

test("compressBound", () => {
  assert.ok(compressBound(1000) >= 1000);
});

test("decompress rejects a wrong size or garbage", () => {
  const data = encode("sized block".repeat(50));
  const compressed = compress(data);
  assert.throws(() => decompress(compressed, data.length - 1));
  assert.throws(() => decompress(compressed, data.length + 1));
  assert.throws(() => decompress(new Uint8Array([0xff, 0xff, 0xff]), 100));
});

test("frame round-trip with options", () => {
  const data = encode("checksummed frame".repeat(100));
  const compressed = compressFrame(data, {
    blockChecksums: true,
    contentChecksum: true,
    contentSize: true,
    linkedBlocks: true,
  });
  assert.deepEqual(
    decompressFrame(compressed, { maxDecompressedSize: data.length }),
    data,
  );
});

test("frame decompression limit applies to full stream", () => {
  const data1 = encode("first bounded frame".repeat(100));
  const data2 = encode("second bounded frame".repeat(100));
  const stream = concat(compressFrame(data1), compressFrame(data2));
  const expected = concat(data1, data2);

  assert.throws(() =>
    decompressFrame(stream, { maxDecompressedSize: expected.length - 1 })
  );
  assert.deepEqual(
    decompressFrame(stream, { maxDecompressedSize: expected.length }),
    expected,
  );
  assert.throws(
    () => decompressFrame(stream, { maxDecompressedSize: -1 }),
    RangeError,
  );
});

test("frame dictionary round-trip and validation", () => {
  const bytes = encode("event_type=metric service=ingest value=".repeat(20));
  const data = encode("event_type=metric service=ingest value=42\n".repeat(50));
  const dict = new Dictionary(bytes, { id: 0xdead_beef });
  const compressed = compressFrame(data, { dictionary: dict });

  assert.deepEqual(decompressFrame(compressed, { dictionary: dict }), data);
  assert.throws(() =>
    decompressFrame(compressed, {
      dictionary: new Dictionary(bytes, { id: 0xbeef_dead }),
    })
  );
  assert.throws(() => new Dictionary(bytes, { id: -1 }), RangeError);

  dict.free();
  assert.throws(() => compressFrame(data, { dictionary: dict }), TypeError);
});

test("dictionary contexts beat plain compression", () => {
  const trainer = new DictTrainer(2048);
  for (let i = 0; i < 200; i++) {
    trainer.addSample(
      encode(`{"ts":"2026-04-27T12:00:00.${i}Z","level":"INFO","status":200}`),
    );
  }
  assert.equal(trainer.sampleCount(), 200);
  const dict = trainer.train();
  assert.ok(dict.length > 0);

  const compressor = Compressor.withDict(dict);
  const decompressor = Decompressor.withDict(dict);
  const data = encode(
    '{"ts":"2026-04-27T12:00:00.7Z","level":"INFO","status":200}',
  );
  const compressed = compressor.compress(data);
  assert.deepEqual(decompressor.decompress(compressed, data.length), data);
  assert.ok(compressed.length < compress(data).length);
  compressor.free();
  decompressor.free();
});

test("stateful contexts without a dictionary", () => {
  const compressor = new Compressor();
  const decompressor = new Decompressor();
  const data = encode("stateful compression".repeat(50));
  const compressed = compressor.compress(data);
  assert.deepEqual(decompressor.decompress(compressed, data.length), data);
  compressor.free();
  decompressor.free();
});

test("package layout serves the WASM next to the resolved entry", () => {
  // Consumers such as OMQ.ts load the bytes this way in Node.
  const require = createRequire(import.meta.url);
  const entry = require.resolve("@paddor/lz4rip");
  const wasmPath = resolve(dirname(entry), "pkg/lz4rip_wasm_bg.wasm");
  assert.ok(existsSync(wasmPath));
  initSyncFromBytes(readFileSync(wasmPath));
});
