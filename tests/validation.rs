use lz4rip::block::{
    DecompressError, DictCompressor, compress, decompress_into, decompress_into_with_dict,
    validate_block, validate_block_with_dict,
};
use proptest::prelude::*;

#[test]
fn truncation_and_trailing_sequences_are_rejected() {
    let input = b"bounded validation checks every compressed byte".repeat(8);
    let compressed = compress(&input);

    for end in 0..compressed.len() {
        assert!(validate_block(&compressed[..end], input.len()).is_err());
    }

    let mut trailing = compressed;
    trailing.push(0);
    assert!(validate_block(&trailing, input.len()).is_err());
}

#[test]
fn mutation_success_still_implies_exact_decompression() {
    let input = b"mutation target mutation target mutation target";
    let compressed = compress(input);

    for index in 0..compressed.len() {
        for bit in [1, 2, 4, 8, 16, 32, 64, 128] {
            let mut mutated = compressed.clone();
            mutated[index] ^= bit;
            if validate_block(&mutated, input.len()).is_ok() {
                let mut output = vec![0; input.len()];
                assert_eq!(decompress_into(&mutated, &mut output), Ok(input.len()));
            }
        }
    }
}

proptest! {
    #[test]
    #[cfg_attr(miri, ignore)]
    fn every_compressed_block_validates(input in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let compressed = compress(&input);
        let validation = validate_block(&compressed, input.len()).unwrap();
        prop_assert_eq!(validation.decoded_len, input.len());

        let mut output = vec![0; input.len()];
        let decoded_len = decompress_into(&compressed, &mut output).unwrap();
        prop_assert_eq!(decoded_len, validation.decoded_len);
        prop_assert_eq!(output, input);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn every_dict_compressed_block_validates(
        input in proptest::collection::vec(any::<u8>(), 0..2048),
        dict in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut compressor = DictCompressor::new(&dict);
        let compressed = compressor.compress(&input);
        let validation = validate_block_with_dict(&compressed, input.len(), &dict).unwrap();
        prop_assert_eq!(validation.decoded_len, input.len());

        let mut output = vec![0; input.len()];
        let decoded_len = decompress_into_with_dict(&compressed, &mut output, &dict).unwrap();
        prop_assert_eq!(decoded_len, validation.decoded_len);
        prop_assert_eq!(output, input);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn validation_success_implies_decompression_success(
        encoded in proptest::collection::vec(any::<u8>(), 0..128),
        expected in 0usize..512,
    ) {
        if let Ok(validation) = validate_block(&encoded, expected) {
            let mut output = vec![0; expected];
            prop_assert_eq!(decompress_into(&encoded, &mut output), Ok(expected));
            prop_assert_eq!(validation.decoded_len, expected);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn dict_validation_success_implies_decompression_success(
        encoded in proptest::collection::vec(any::<u8>(), 0..128),
        expected in 0usize..512,
        dict in proptest::collection::vec(any::<u8>(), 0..128),
    ) {
        if let Ok(validation) = validate_block_with_dict(&encoded, expected, &dict) {
            let mut output = vec![0; expected];
            prop_assert_eq!(
                decompress_into_with_dict(&encoded, &mut output, &dict),
                Ok(expected)
            );
            prop_assert_eq!(validation.decoded_len, expected);
        }
    }
}

#[test]
fn size_mismatch_is_distinct_from_output_capacity() {
    assert_eq!(
        validate_block(&[0x10, b'x'], 2),
        Err(DecompressError::DecodedSizeMismatch {
            expected: 2,
            actual: 1,
        })
    );
}
