import {
  assert,
  assertEquals,
  assertThrows,
} from "https://deno.land/std@0.224.0/assert/mod.ts";
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
} from "./mod.ts";

function concatArrays(...parts: Uint8Array[]): Uint8Array {
  const length = parts.reduce((sum, part) => sum + part.length, 0);
  const output = new Uint8Array(length);
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.length;
  }
  return output;
}

Deno.test("init", async () => {
  await init();
});

// --- One-shot ---

Deno.test("one-shot round-trip", () => {
  const data = new TextEncoder().encode("hello world, hello lz4!".repeat(100));
  const compressed = compress(data);
  assert(compressed.length < data.length);
  const decompressed = decompress(compressed, data.length);
  assertEquals(decompressed, data);
});

Deno.test("compressBound", () => {
  const data = new TextEncoder().encode("compress bound payload".repeat(50));
  const bound = compressBound(data.length);
  assert(bound >= data.length);
  assert(compress(data).length <= bound);
});

Deno.test("empty input", () => {
  const empty = new Uint8Array(0);
  const compressed = compress(empty);
  const decompressed = decompress(compressed, 0);
  assertEquals(decompressed, empty);
});

Deno.test("incompressible data", () => {
  const random = crypto.getRandomValues(new Uint8Array(4096));
  const compressed = compress(random);
  const decompressed = decompress(compressed, random.length);
  assertEquals(decompressed, random);
});

// --- Frame format ---

Deno.test("frame round-trip", () => {
  const data = new TextEncoder().encode("hello frame lz4".repeat(100));
  const compressed = compressFrame(data);
  const decompressed = decompressFrame(compressed);
  assertEquals(decompressed, data);
});

Deno.test("frame supports options", () => {
  const data = new TextEncoder().encode("checksummed frame".repeat(100));
  const compressed = compressFrame(data, {
    blockChecksums: true,
    contentChecksum: true,
    contentSize: true,
    linkedBlocks: true,
  });
  const decompressed = decompressFrame(compressed, {
    maxDecompressedSize: data.length,
  });
  assertEquals(decompressed, data);
});

Deno.test("frame decompresses concatenated frames", () => {
  const data1 = new TextEncoder().encode("first frame".repeat(100));
  const data2 = new TextEncoder().encode("second frame".repeat(100));
  const stream = concatArrays(compressFrame(data1), compressFrame(data2));
  assertEquals(decompressFrame(stream), concatArrays(data1, data2));
});

Deno.test("frame decompression limit applies to full stream", () => {
  const data1 = new TextEncoder().encode("first bounded frame".repeat(100));
  const data2 = new TextEncoder().encode("second bounded frame".repeat(100));
  const stream = concatArrays(compressFrame(data1), compressFrame(data2));
  const expected = concatArrays(data1, data2);

  assertThrows(
    () => decompressFrame(stream, { maxDecompressedSize: expected.length - 1 }),
    Error,
  );
  assertEquals(
    decompressFrame(stream, { maxDecompressedSize: expected.length }),
    expected,
  );
});

Deno.test("frame dictionary round-trip", () => {
  const dictionary = new TextEncoder().encode(
    "event_type=metric service=ingest shard= value=".repeat(20),
  );
  const data = new TextEncoder().encode(
    "event_type=metric service=ingest shard=3 value=42\n".repeat(100),
  );
  const dict = new Dictionary(dictionary, { id: 0xdead_beef });
  const compressed = compressFrame(data, {
    dictionary: dict,
    linkedBlocks: true,
    contentChecksum: true,
    contentSize: true,
  });

  assertEquals(decompressFrame(compressed, { dictionary: dict }), data);
  assertThrows(
    () =>
      decompressFrame(compressed, {
        dictionary: new Dictionary(dictionary, { id: 0xbeef_dead }),
      }),
    Error,
  );

  dict.free();
});

Deno.test("frame dictionary validates id and lifetime", () => {
  const dictionary = new TextEncoder().encode("dictionary");
  const data = new TextEncoder().encode("data");

  assertThrows(
    () => new Dictionary(dictionary, { id: -1 }),
    RangeError,
  );
  assertThrows(
    () => new Dictionary(dictionary, { id: 0x1_0000_0000 }),
    RangeError,
  );

  const dict = new Dictionary(dictionary, { id: 1 });
  dict.free();

  assertThrows(
    () => compressFrame(data, { dictionary: dict }),
    TypeError,
  );
  assertThrows(
    () => decompressFrame(compressFrame(data), { dictionary: dict }),
    TypeError,
  );
});

// --- Stateful ---

Deno.test("stateful compressor", () => {
  const compressor = new Compressor();
  const data1 = new TextEncoder().encode("first message".repeat(50));
  const data2 = new TextEncoder().encode("second message".repeat(50));

  const c1 = compressor.compress(data1);
  const c2 = compressor.compress(data2);

  assertEquals(decompress(c1, data1.length), data1);
  assertEquals(decompress(c2, data2.length), data2);

  compressor.free();
});

Deno.test("stateful decompressor", () => {
  const data1 = new TextEncoder().encode("decompress test 1".repeat(50));
  const data2 = new TextEncoder().encode("decompress test 2".repeat(50));

  const c1 = compress(data1);
  const c2 = compress(data2);

  const decompressor = new Decompressor();
  assertEquals(decompressor.decompress(c1, data1.length), data1);
  assertEquals(decompressor.decompress(c2, data2.length), data2);

  decompressor.free();
});

// --- Dictionary ---

Deno.test("dict round-trip", () => {
  const dict = new TextEncoder().encode(
    '{"ts":"2026-04-27","level":"INFO","service":"api"}'.repeat(20),
  );

  const compressor = Compressor.withDict(dict);
  const decompressor = Decompressor.withDict(dict);

  const data = new TextEncoder().encode(
    '{"ts":"2026-04-27","level":"INFO","service":"api","msg":"ok"}'.repeat(10),
  );
  const compressed = compressor.compress(data);
  const decompressed = decompressor.decompress(compressed, data.length);
  assertEquals(decompressed, data);

  const compressedPlain = compress(data);
  assert(
    compressed.length < compressedPlain.length,
    `dict ${compressed.length} should beat plain ${compressedPlain.length}`,
  );

  compressor.free();
  decompressor.free();
});

Deno.test("dict stateful contexts reuse dictionary", () => {
  const dict = new TextEncoder().encode(
    '{"ts":"2026-04-27","level":"INFO","service":"api"}'.repeat(20),
  );
  const compressor = Compressor.withDict(dict);
  const decompressor = Decompressor.withDict(dict);
  const data1 = new TextEncoder().encode(
    '{"ts":"2026-04-27","level":"INFO","service":"api","msg":"ok1"}'.repeat(
      10,
    ),
  );
  const data2 = new TextEncoder().encode(
    '{"ts":"2026-04-27","level":"INFO","service":"api","msg":"ok2"}'.repeat(
      10,
    ),
  );

  const c1 = compressor.compress(data1);
  const c2 = compressor.compress(data2);

  assertEquals(decompressor.decompress(c1, data1.length), data1);
  assertEquals(decompressor.decompress(c2, data2.length), data2);

  compressor.free();
  decompressor.free();
});

Deno.test("dict trainer", () => {
  const trainer = new DictTrainer(2048);
  for (let i = 0; i < 200; i++) {
    trainer.addSample(
      new TextEncoder().encode(
        `{"ts":"2026-04-27T12:00:00.${i}Z","level":"INFO","service":"api-gw","status":200}`,
      ),
    );
  }
  assertEquals(trainer.sampleCount(), 200);
  const dict = trainer.train();
  assert(dict.length > 0);
});

Deno.test("dict trainer consumes on train", () => {
  const trainer = new DictTrainer(1024);
  for (let i = 0; i < 50; i++) {
    trainer.addSample(new TextEncoder().encode(`sample ${i} data`.repeat(5)));
  }
  trainer.train();
  assertThrows(
    () => trainer.addSample(new TextEncoder().encode("late sample")),
    Error,
  );
  assertThrows(() => trainer.sampleCount(), Error);
  assertThrows(() => trainer.train(), Error);
});

// --- Error paths ---

Deno.test("decompress with too-small size throws", () => {
  const data = new TextEncoder().encode("hello world".repeat(100));
  const compressed = compress(data);
  assertThrows(
    () => decompress(compressed, data.length - 100),
    Error,
  );
});

Deno.test("decompress with too-large size throws", () => {
  const data = new TextEncoder().encode("hello world".repeat(100));
  const compressed = compress(data);
  assertThrows(
    () => decompress(compressed, data.length + 100),
    Error,
  );
});

Deno.test("decompress truncated data throws", () => {
  const data = new TextEncoder().encode("hello world".repeat(100));
  const compressed = compress(data);
  assertThrows(
    () => decompress(compressed.slice(0, compressed.length / 2), data.length),
    Error,
  );
});

Deno.test("decompress corrupted data throws", () => {
  const data = new TextEncoder().encode("hello world".repeat(100));
  const compressed = compress(data);
  const corrupted = new Uint8Array(compressed);
  corrupted[0] = 0xff;
  assertThrows(
    () => decompress(corrupted, data.length),
    Error,
  );
});

Deno.test("decompress garbage throws", () => {
  assertThrows(
    () => decompress(new Uint8Array([0, 1, 2, 3, 4, 5]), 100),
    Error,
  );
});
