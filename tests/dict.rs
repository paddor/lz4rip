use lz4rip::block::{
    Decompressor, DecompressorRef, DictCompressor, DictCompressorRef, DictTrainer, compress,
    compress_into_with_dict, decompress_into_with_dict, get_maximum_output_size,
};
use more_asserts::assert_lt;

fn json_payload(target_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bytes);
    let mut i = 0u64;
    while out.len() < target_bytes {
        let line = format!(
            r#"{{"ts":1700000000,"level":"INFO","service":"ingest","event":{},"message":"repeatable structured payload for lz4 dictionary tests"}}"#,
            i
        );
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
        i += 1;
    }
    out.truncate(target_bytes);
    out
}

fn dict_roundtrip(corpus: &[u8], dict_bytes: usize) {
    let dict = &corpus[..dict_bytes];
    let input = &corpus[dict_bytes..];

    let mut comp = DictCompressor::new(dict);
    let compressed = comp.compress(input);
    let decomp = Decompressor::with_dict(dict);
    let decompressed = decomp.decompress(&compressed, input.len()).unwrap();
    assert_eq!(input, &decompressed[..]);

    let no_dict = compress(input);
    assert_lt!(compressed.len(), no_dict.len());
}

#[test]
#[cfg_attr(miri, ignore)]
fn generated_json_dict_4k() {
    let corpus = json_payload(66_675);
    dict_roundtrip(&corpus, 4096);
}

#[test]
#[cfg_attr(miri, ignore)]
fn generated_json_dict_64k() {
    let corpus = json_payload(132_000);
    dict_roundtrip(&corpus, 65536);
}

#[test]
fn json66k_dict_4k() {
    let corpus = json_payload(66_675);
    dict_roundtrip(&corpus, 4096);
}

#[test]
fn dict_trainer_zero_samples() {
    let trainer = DictTrainer::new(2048);
    assert!(trainer.train().is_empty());
}

#[test]
fn dict_trainer_one_sample() {
    let mut trainer = DictTrainer::new(2048);
    trainer.add_sample(b"hello world");
    assert!(trainer.train().is_empty());
}

#[test]
fn dict_trainer_short_samples_skipped() {
    let mut trainer = DictTrainer::new(2048);
    trainer.add_sample(b"ab");
    trainer.add_sample(b"cd");
    assert_eq!(trainer.sample_count(), 0);
}

#[test]
fn dict_trainer_oversized_sample_skipped() {
    let mut trainer = DictTrainer::new(16);
    let big = vec![b'x'; 32];
    trainer.add_sample(&big);
    assert_eq!(trainer.sample_count(), 0);
}

#[test]
fn dict_trainer_produces_usable_dict() {
    let mut trainer = DictTrainer::new(2048);
    for i in 0..100u8 {
        let sample = format!("message id={i} payload=the quick brown fox");
        trainer.add_sample(sample.as_bytes());
    }
    let dict = trainer.train();
    assert!(!dict.is_empty());
    assert!(dict.len() <= 2048);

    let input = b"message id=42 payload=the quick brown fox jumps";
    let mut comp = DictCompressor::new(&dict);
    let compressed = comp.compress(input);
    let decomp = Decompressor::with_dict(&dict);
    let decompressed = decomp.decompress(&compressed, input.len()).unwrap();
    assert_eq!(&decompressed[..], input);
}

// ---- Adversarial dict tests (all miri-friendly) ----

fn dict_roundtrip_ref(dict: &[u8], input: &[u8]) {
    let mut comp = DictCompressorRef::new(dict);
    let mut out = vec![0u8; get_maximum_output_size(input.len())];
    let n = comp.compress_into(input, &mut out).unwrap();
    let compressed = &out[..n];

    let decomp = DecompressorRef::with_dict(dict);
    let decompressed = decomp.decompress(compressed, input.len()).unwrap();
    assert_eq!(&decompressed[..], input);
}

fn dict_roundtrip_owning(dict: &[u8], input: &[u8]) {
    let mut comp = DictCompressor::new(dict);
    let compressed = comp.compress(input);
    let decomp = Decompressor::with_dict(dict);
    let decompressed = decomp.decompress(&compressed, input.len()).unwrap();
    assert_eq!(&decompressed[..], input);
}

fn dict_roundtrip_free(dict: &[u8], input: &[u8]) {
    let mut out = vec![0u8; get_maximum_output_size(input.len())];
    let n = compress_into_with_dict(input, &mut out, dict).unwrap();
    let mut dec_out = vec![0u8; input.len()];
    let m = decompress_into_with_dict(&out[..n], &mut dec_out, dict).unwrap();
    assert_eq!(&dec_out[..m], input);
}

/// Dict below MINMATCH (4): should be ignored, still roundtrips.
#[test]
fn dict_too_short_ignored() {
    let dict = b"abc";
    let input = b"hello world, hello world, hello!";
    dict_roundtrip_ref(dict, input);
    dict_roundtrip_owning(dict, input);
    dict_roundtrip_free(dict, input);
}

/// Dict exactly MINMATCH (4 bytes).
#[test]
fn dict_exactly_minmatch() {
    let dict = b"helo";
    let input = b"helo world helo world helo helo";
    dict_roundtrip_ref(dict, input);
    dict_roundtrip_owning(dict, input);
}

/// Empty input with a dict.
#[test]
fn dict_empty_input() {
    let dict = b"some dictionary content here padding";
    let input = b"";
    dict_roundtrip_ref(dict, input);
    dict_roundtrip_owning(dict, input);
    dict_roundtrip_free(dict, input);
}

/// Input shorter than MINMATCH with a dict.
#[test]
fn dict_input_shorter_than_minmatch() {
    let dict = b"abcdefghijklmnop";
    let input = b"ab";
    dict_roundtrip_ref(dict, input);
    dict_roundtrip_owning(dict, input);
}

/// Match that spans the dict/input boundary: input starts with bytes from the
/// dict, forcing the compressor to emit a match whose source is in the dict.
#[test]
fn dict_match_crosses_boundary() {
    let dict = b"the quick brown fox jumps over the lazy dog";
    let input = b"the lazy dog sleeps, the quick brown fox jumps over the lazy dog again";
    dict_roundtrip_ref(dict, input);
    dict_roundtrip_owning(dict, input);
    dict_roundtrip_free(dict, input);
}

/// Reuse: compress many small inputs through the same owning Compressor.
/// Catches use-after-free on the self-referential dict borrow.
#[test]
fn dict_compressor_reuse() {
    let dict = b"prefix:key=value;prefix:key=value;padding!!";
    let mut comp = DictCompressor::new(dict);
    let decomp = Decompressor::with_dict(dict);

    for i in 0u8..50 {
        let input: Vec<u8> = format!("prefix:key=value;seq={i};extra padding data").into();
        let compressed = comp.compress(&input);
        let decompressed = decomp.decompress(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);
    }
}

/// Reuse: CompressorRef with dict, many calls.
#[test]
fn dict_compressor_ref_reuse() {
    let dict = b"AAAA BBBB CCCC DDDD EEEE FFFF GGGG HHHH";
    let mut comp = DictCompressorRef::new(dict);
    let decomp = DecompressorRef::with_dict(dict);

    for i in 0u8..50 {
        let input: Vec<u8> = [b"AAAA BBBB CCCC " as &[u8], &[i], b" DDDD EEEE"].concat();
        let mut out = vec![0u8; get_maximum_output_size(input.len())];
        let n = comp.compress_into(&input, &mut out).unwrap();
        let decompressed = decomp.decompress(&out[..n], input.len()).unwrap();
        assert_eq!(decompressed, input);
    }
}

/// Dict larger than input: exercises the dual-table path where dict entries
/// dominate the hash table.
#[test]
fn dict_larger_than_input() {
    let dict: Vec<u8> = (0..512)
        .map(|i| b"the quick brown fox jumps "[i % 26])
        .collect();
    let input = b"the quick brown fox";
    dict_roundtrip_ref(&dict, input);
    dict_roundtrip_owning(&dict, input);
}

/// All-zeros dict + all-zeros input: overlapping match copies with offset=1
/// in the dict path.
#[test]
fn dict_all_zeros() {
    let dict = vec![0u8; 256];
    let input = vec![0u8; 300];
    dict_roundtrip_ref(&dict, &input);
    dict_roundtrip_owning(&dict, &input);
}

/// Highly repetitive dict + varied input: dict fills the hash table with
/// entries that all collide, then input has different content.
#[test]
fn dict_repetitive_input_varied() {
    let dict: Vec<u8> = b"ABCDABCDABCDABCD".repeat(8);
    let input: Vec<u8> = (0u8..=255).collect();
    dict_roundtrip_ref(&dict, &input);
    dict_roundtrip_owning(&dict, &input);
}

/// Dict + input near the u16 boundary (dict 32K, input 32K).
/// Exercises the fallback from dual HashTableU32U16 to single HashTableU32.
#[test]
#[cfg_attr(miri, ignore)]
fn dict_u16_boundary() {
    let dict: Vec<u8> = (0u8..=255).cycle().take(32768).collect();
    let input: Vec<u8> = (0u8..=255).cycle().take(32768).collect();
    dict_roundtrip_owning(&dict, &input);
}

/// Dict + input just below u16 boundary: stays on the dual-table fast path.
#[test]
fn dict_just_below_u16() {
    let dict: Vec<u8> = (0u8..=255).cycle().take(16384).collect();
    let input: Vec<u8> = (0u8..=255).cycle().take(16384).collect();
    dict_roundtrip_ref(&dict, &input);
    dict_roundtrip_owning(&dict, &input);
}

/// Decompress with dict + corrupted compressed data: must not UB.
#[test]
fn dict_decompress_corrupted() {
    let dict = b"dictionary content for decompression test here!!";
    let input = b"dictionary content appears in input too here!!";
    let mut comp = DictCompressor::new(dict);
    let compressed = comp.compress(input);

    let decomp = Decompressor::with_dict(dict);
    for i in 0..compressed.len() {
        let mut corrupted = compressed.clone();
        corrupted[i] ^= 0xFF;
        let _ = decomp.decompress(&corrupted, input.len());
    }
    for i in 1..compressed.len() {
        let _ = decomp.decompress(&compressed[..i], input.len());
    }
}

/// Decompress with wrong dict: must error, not UB.
#[test]
fn dict_decompress_wrong_dict() {
    let dict_a = b"aaaa bbbb cccc dddd eeee ffff gggg hhhh";
    let dict_b = b"xxxx yyyy zzzz wwww vvvv uuuu tttt ssss";
    let input = b"aaaa bbbb cccc dddd, aaaa bbbb cccc dddd";

    let mut comp = DictCompressor::new(dict_a);
    let compressed = comp.compress(input);

    let decomp = Decompressor::with_dict(dict_b);
    let result = decomp.decompress(&compressed, input.len());
    assert!(result.is_err() || result.unwrap() != input);
}

/// Free-function dict API with various sizes.
#[test]
fn dict_free_fn_sizes() {
    let dict = b"common prefix string for matching purposes here";
    for size in [0, 1, 3, 4, 5, 15, 16, 50, 100, 255, 256, 500] {
        let input: Vec<u8> = (0u8..=255).cycle().take(size).collect();
        dict_roundtrip_free(dict, &input);
    }
}
