#![no_main]
use libfuzzer_sys::fuzz_target;

use lz4rip::block::{Decompressor, DictCompressor, RatioDictCompressor};

/// Round-trip with external dictionary. Targets:
/// - Dict exactly at WINDOW_SIZE (65536) boundary
/// - Dict crossing match offset boundaries
/// - copy_from_dict: match starting in dict, crossing into output
/// - Dict shorter than MINMATCH (4 bytes) - should be rejected or handled
#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    data: Vec<u8>,
    dict: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(1..=4u8))]
    repeat: u8,
}

fuzz_target!(|input: Input| {
    let mut payload = Vec::with_capacity(input.data.len() * input.repeat as usize);
    for _ in 0..input.repeat {
        payload.extend_from_slice(&input.data);
    }
    if payload.is_empty() {
        return;
    }

    // Truncate dict to reasonable size (up to 2x WINDOW_SIZE to test trimming).
    let dict_len = input.dict.len().min(128 * 1024);
    let dict = &input.dict[..dict_len];

    let mut compressor = DictCompressor::new(dict);
    let decompressor = Decompressor::with_dict(dict);

    let compressed = compressor.compress(&payload);
    let decompressed = decompressor.decompress(&compressed, payload.len()).unwrap();
    assert_eq!(payload, decompressed);

    // Into-buffer variant
    let max_out = lz4rip::block::get_maximum_output_size(payload.len());
    let mut comp_buf = vec![0u8; max_out];
    let comp_len = compressor.compress_into(&payload, &mut comp_buf).unwrap();
    let mut decomp_buf = vec![0u8; payload.len()];
    let decomp_len = decompressor
        .decompress_into(&comp_buf[..comp_len], &mut decomp_buf)
        .unwrap();
    assert_eq!(&payload[..], &decomp_buf[..decomp_len]);

    // Ratio profile, twice through one compressor: its working table must
    // not keep anything from the first input.
    let mut ratio = RatioDictCompressor::new(dict);
    let first = ratio.compress(&payload);
    assert_eq!(ratio.compress(&payload), first);
    let decompressed = decompressor.decompress(&first, payload.len()).unwrap();
    assert_eq!(payload, decompressed);

    // Cross-check: decompress WITHOUT dict should fail or produce wrong output
    // (must not panic either way).
    let _ = lz4rip::block::decompress(&compressed, payload.len());
});
