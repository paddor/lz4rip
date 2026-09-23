//! No-output LZ4 block validation.

use lz4rip_core::DecompressError;

use crate::decompress::{read_literal_length, read_match_length, read_offset};

/// Result of validating a raw LZ4 block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockValidation {
    /// Exact number of bytes produced by decoding the block.
    pub decoded_len: usize,
}

#[inline]
fn extension_limit(decoded_pos: usize, base_length: usize, expected: usize) -> usize {
    expected
        .saturating_sub(decoded_pos)
        .saturating_sub(base_length)
}

#[inline]
fn advance_decoded(
    decoded_pos: usize,
    length: usize,
    expected: usize,
) -> Result<usize, DecompressError> {
    let actual = decoded_pos
        .checked_add(length)
        .ok_or(DecompressError::LiteralOutOfBounds)?;
    if actual > expected {
        return Err(DecompressError::DecodedSizeMismatch { expected, actual });
    }
    Ok(actual)
}

#[inline]
fn validate_block_internal(
    input: &[u8],
    expected_decoded_len: usize,
    dict_len: usize,
) -> Result<BlockValidation, DecompressError> {
    let mut input_pos = 0;
    let mut decoded_pos = 0;

    loop {
        let token = *input
            .get(input_pos)
            .ok_or(DecompressError::ExpectedAnotherByte)?;
        input_pos += 1;

        let literal_extension_limit = extension_limit(decoded_pos, 15, expected_decoded_len);
        let literal_length =
            read_literal_length(input, &mut input_pos, token, literal_extension_limit)?;
        decoded_pos = advance_decoded(decoded_pos, literal_length, expected_decoded_len)?;

        input_pos = input_pos
            .checked_add(literal_length)
            .filter(|&end| end <= input.len())
            .ok_or(DecompressError::LiteralOutOfBounds)?;

        if input_pos == input.len() {
            break;
        }

        let offset = read_offset(input, &mut input_pos)?;

        let match_start = decoded_pos;
        let match_extension_limit = extension_limit(
            decoded_pos,
            lz4rip_core::MINMATCH + 15,
            expected_decoded_len,
        );
        let match_length = read_match_length(input, &mut input_pos, token, match_extension_limit)?;
        decoded_pos = advance_decoded(decoded_pos, match_length, expected_decoded_len)?;

        if offset > match_start && offset - match_start > dict_len {
            return Err(DecompressError::OffsetOutOfBounds);
        }
    }

    if decoded_pos != expected_decoded_len {
        return Err(DecompressError::DecodedSizeMismatch {
            expected: expected_decoded_len,
            actual: decoded_pos,
        });
    }

    Ok(BlockValidation {
        decoded_len: decoded_pos,
    })
}

/// Validate a raw LZ4 block and its exact decoded length without producing
/// decoded bytes.
///
/// This validates block syntax, input consumption, match offsets, and decoded
/// size. It does not validate application-level meaning of the decoded bytes.
/// Memory use is independent of `expected_decoded_len`.
#[inline]
pub fn validate_block(
    input: &[u8],
    expected_decoded_len: usize,
) -> Result<BlockValidation, DecompressError> {
    validate_block_internal(input, expected_decoded_len, 0)
}

/// Validate a raw LZ4 block and its exact decoded length using an external
/// dictionary, without producing decoded bytes.
///
/// Match contents do not affect block syntax, so validation reads only the
/// dictionary length. Passing the dictionary slice keeps validation and actual
/// decompression tied to the same retained history.
#[inline]
pub fn validate_block_with_dict(
    input: &[u8],
    expected_decoded_len: usize,
    dict: &[u8],
) -> Result<BlockValidation, DecompressError> {
    validate_block_internal(input, expected_decoded_len, dict.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_literals_and_matches() {
        assert_eq!(
            validate_block(&[0], 0),
            Ok(BlockValidation { decoded_len: 0 })
        );
        assert_eq!(
            validate_block(&[0x30, b'a', b'b', b'c'], 3),
            Ok(BlockValidation { decoded_len: 3 })
        );
        assert_eq!(
            validate_block(&[0x10, b'a', 1, 0, 0], 5),
            Ok(BlockValidation { decoded_len: 5 })
        );
    }

    #[test]
    fn validates_dictionary_matches() {
        let block = [0, 1, 0, 0];
        assert_eq!(
            validate_block_with_dict(&block, 4, b"x"),
            Ok(BlockValidation { decoded_len: 4 })
        );
        assert_eq!(
            validate_block_with_dict(&[0x10, b'a', 2, 0, 0], 5, b"x"),
            Ok(BlockValidation { decoded_len: 5 })
        );
        assert_eq!(
            validate_block_with_dict(&block, 4, b""),
            Err(DecompressError::OffsetOutOfBounds)
        );
    }

    #[test]
    fn rejects_wrong_decoded_size() {
        assert_eq!(
            validate_block(&[0x10, b'a'], 2),
            Err(DecompressError::DecodedSizeMismatch {
                expected: 2,
                actual: 1,
            })
        );
        assert_eq!(
            validate_block(&[0x10, b'a'], 0),
            Err(DecompressError::DecodedSizeMismatch {
                expected: 0,
                actual: 1,
            })
        );
    }

    #[test]
    fn excessive_extensions_stop_at_size_bound() {
        assert!(matches!(
            validate_block(&[0xF0, 0xFF], 1),
            Err(DecompressError::DecodedSizeMismatch { .. })
        ));
        assert!(matches!(
            validate_block(&[0x1F, b'a', 1, 0, 0xFF], 4),
            Err(DecompressError::DecodedSizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_malformed_blocks() {
        assert_eq!(
            validate_block(&[], 0),
            Err(DecompressError::ExpectedAnotherByte)
        );
        assert_eq!(
            validate_block(&[0x20, b'a'], 2),
            Err(DecompressError::LiteralOutOfBounds)
        );
        assert_eq!(
            validate_block(&[0, 0], 4),
            Err(DecompressError::ExpectedAnotherByte)
        );
        assert_eq!(
            validate_block(&[0, 0, 0], 4),
            Err(DecompressError::OffsetZero)
        );
        assert_eq!(
            validate_block(&[0, 1, 0, 0], 4),
            Err(DecompressError::OffsetOutOfBounds)
        );
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn decoded_advance_is_checked_and_bounded() {
        let decoded_pos: usize = kani::any();
        let length: usize = kani::any();
        let expected: usize = kani::any();

        if let Ok(actual) = advance_decoded(decoded_pos, length, expected) {
            assert_eq!(decoded_pos.checked_add(length), Some(actual));
            assert!(actual <= expected);
        }
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn validate_4byte_no_overflow() {
        let input: [u8; 4] = kani::any();
        let expected: u8 = kani::any();
        let _ = validate_block(&input, expected as usize);
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn validate_dict_4byte_no_overflow() {
        let input: [u8; 4] = kani::any();
        let expected: u8 = kani::any();
        let dict: [u8; 4] = kani::any();
        let _ = validate_block_with_dict(&input, expected as usize, &dict);
    }
}
