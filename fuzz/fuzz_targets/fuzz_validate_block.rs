#![no_main]

use libfuzzer_sys::fuzz_target;
use lz4rip::block::{compress, decompress_into, validate_block};

#[derive(Debug, arbitrary::Arbitrary)]
struct FuzzData {
    encoded: Vec<u8>,
    payload: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(0..=65535usize))]
    expected: usize,
    mutation_index: usize,
    mutation_mask: u8,
    truncation: usize,
}

fn check_implication(encoded: &[u8], expected: usize) {
    if let Ok(validation) = validate_block(encoded, expected) {
        let mut output = vec![0; expected];
        assert_eq!(decompress_into(encoded, &mut output), Ok(expected));
        assert_eq!(validation.decoded_len, expected);
    }
}

fuzz_target!(|data: FuzzData| {
    check_implication(&data.encoded, data.expected);

    let compressed = compress(&data.payload);
    assert_eq!(
        validate_block(&compressed, data.payload.len())
            .unwrap()
            .decoded_len,
        data.payload.len()
    );

    let mut mutated = compressed.clone();
    let index = data.mutation_index % mutated.len();
    mutated[index] ^= data.mutation_mask;
    check_implication(&mutated, data.payload.len());

    let end = data.truncation % compressed.len();
    check_implication(&compressed[..end], data.payload.len());
});
