#![no_main]
use libfuzzer_sys::fuzz_target;

use lz4rip::block::{decompress, decompress_into, Decompressor};

#[derive(Debug, arbitrary::Arbitrary)]
struct FuzzData {
    input: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(0..=65535usize))]
    output_size: usize,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(0..=65536usize))]
    dict_size: usize,
    dict_pattern: u8,
}

fuzz_target!(|fuzz_data: FuzzData| {
    let input = &fuzz_data.input;
    let output_size = fuzz_data.output_size;

    // Non-trivial dict: repeated pattern so dict matches are structurally interesting.
    let dict: Vec<u8> = (0..fuzz_data.dict_size)
        .map(|i| fuzz_data.dict_pattern.wrapping_add(i as u8))
        .collect();

    // Allocating decompress: must not panic.
    let result = if dict.is_empty() {
        decompress(input, output_size)
    } else {
        Decompressor::with_dict(&dict).decompress(input, output_size)
    };
    // The Vec API decodes like the into-buffer API (it skips the zero fill
    // for literal-only blocks).
    let mut out = vec![0u8; output_size];
    let into = if dict.is_empty() {
        decompress_into(input, &mut out)
    } else {
        Decompressor::with_dict(&dict).decompress_into(input, &mut out)
    };
    assert_eq!(result, into.map(|len| out[..len].to_vec()));

    // Into-buffer decompress: must not panic even with exact/tight buffers.
    for buf_size in [
        output_size,
        output_size.saturating_add(1),
        output_size.saturating_sub(1),
    ] {
        if buf_size == 0 && input.is_empty() {
            continue;
        }
        let mut out = vec![0u8; buf_size];
        let result = if dict.is_empty() {
            decompress_into(input, &mut out)
        } else {
            Decompressor::with_dict(&dict).decompress_into(input, &mut out)
        };
        if let Ok(len) = result {
            for &byte in &out[..len] {
                std::hint::black_box(byte);
            }
        }
    }
});
