#![no_main]
use libfuzzer_sys::fuzz_target;

use lz4rip::block::{compress_into, decompress, decompress_into, get_maximum_output_size};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    data: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(1..=8u8))]
    repeat: u8,
    /// Chunk size for the reused `RatioCompressor`.
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(1..=600u16))]
    chunk: u16,
    /// Passes over the chunks: up to 96 KiB through one compressor, enough
    /// to wrap its small table's `u16` positions.
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(1..=3u8))]
    rounds: u8,
}

fuzz_target!(|input: Input| {
    // Repeat data to cross the 64KB hash table boundary (HashTable4KU16 → HashTable4K).
    let mut payload = Vec::with_capacity(input.data.len() * input.repeat as usize);
    for _ in 0..input.repeat {
        payload.extend_from_slice(&input.data);
    }

    // compress + decompress API
    let compressed = lz4rip::compress(&payload);
    let decompressed = decompress(&compressed, payload.len()).unwrap();
    assert_eq!(payload, decompressed);

    // into-buffer API: exact-size output buffer
    let max_out = get_maximum_output_size(payload.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = compress_into(&payload, &mut comp_buf).unwrap();
    let mut decomp_buf = vec![0u8; payload.len()];
    let decomp_len = decompress_into(&comp_buf[..comp_len], &mut decomp_buf).unwrap();
    assert_eq!(&payload[..], &decomp_buf[..decomp_len]);

    // ratio profile: one-shot and reusable compressor give the same output
    let ratio = lz4rip::compress_ratio(&payload);
    assert_eq!(decompress(&ratio, payload.len()).unwrap(), payload);
    let mut comp = lz4rip::block::RatioCompressor::new();
    assert_eq!(comp.compress(&payload), ratio);

    // One compressor over many chunks, as in a hot loop: chunks up to 256 B
    // reuse its table, larger ones clear it.
    for _ in 0..input.rounds {
        for c in payload.chunks(input.chunk as usize) {
            assert_eq!(comp.compress(c), lz4rip::compress_ratio(c));
        }
    }
});
