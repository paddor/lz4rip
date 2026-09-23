#![no_main]

use libfuzzer_sys::fuzz_target;
use lz4rip::block::{
    DictCompressor, decompress_into_with_dict, validate_block_with_dict,
};

#[derive(Debug, arbitrary::Arbitrary)]
struct FuzzData {
    encoded: Vec<u8>,
    payload: Vec<u8>,
    dict: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(0..=65535usize))]
    expected: usize,
    mutation_index: usize,
    mutation_mask: u8,
    truncation: usize,
}

fn check_implication(encoded: &[u8], expected: usize, dict: &[u8]) {
    if let Ok(validation) = validate_block_with_dict(encoded, expected, dict) {
        let mut output = vec![0; expected];
        assert_eq!(
            decompress_into_with_dict(encoded, &mut output, dict),
            Ok(expected)
        );
        assert_eq!(validation.decoded_len, expected);
    }
}

fuzz_target!(|data: FuzzData| {
    let dict_len = data.dict.len().min(128 * 1024);
    let dict = &data.dict[..dict_len];
    check_implication(&data.encoded, data.expected, dict);

    let mut compressor = DictCompressor::new(dict);
    let compressed = compressor.compress(&data.payload);
    assert_eq!(
        validate_block_with_dict(&compressed, data.payload.len(), dict)
            .unwrap()
            .decoded_len,
        data.payload.len()
    );

    let mut mutated = compressed.clone();
    let index = data.mutation_index % mutated.len();
    mutated[index] ^= data.mutation_mask;
    check_implication(&mutated, data.payload.len(), dict);

    let end = data.truncation % compressed.len();
    check_implication(&compressed[..end], data.payload.len(), dict);
});
