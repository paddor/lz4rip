#![no_main]
use libfuzzer_sys::fuzz_target;
use std::mem::MaybeUninit;

use lz4rip::block::{
    decompress_append, decompress_into, decompress_into_uninit, DecompressError, Decompressor,
};

#[derive(Debug, arbitrary::Arbitrary)]
struct FuzzData {
    input: Vec<u8>,
    #[arbitrary(with = |u: &mut arbitrary::Unstructured| u.int_in_range(0..=65536usize))]
    dict_size: usize,
    dict_byte: u8,
}

fuzz_target!(|fuzz_data: FuzzData| {
    let input = &fuzz_data.input;
    // Dict with non-trivial content so dict-match copies are exercised.
    let dict: Vec<u8> = (0..fuzz_data.dict_size)
        .map(|i| fuzz_data.dict_byte.wrapping_add(i as u8))
        .collect();

    let buf_size = 512.max(input.len() * 4);

    fn decompress(input: &[u8], output: &mut [u8], dict: &[u8]) -> Result<usize, DecompressError> {
        if dict.is_empty() {
            decompress_into(input, output)
        } else {
            Decompressor::with_dict(dict).decompress_into(input, output)
        }
    }

    fn decompress_uninit(
        input: &[u8],
        output: &mut [MaybeUninit<u8>],
        dict: &[u8],
    ) -> Result<Vec<u8>, DecompressError> {
        let out = if dict.is_empty() {
            decompress_into_uninit(input, output)?
        } else {
            Decompressor::with_dict(dict).decompress_into_uninit(input, output)?
        };
        Ok(out.to_vec())
    }

    fn append(
        input: &[u8],
        output: &mut Vec<u8>,
        max_size: usize,
        dict: &[u8],
    ) -> Result<usize, DecompressError> {
        if dict.is_empty() {
            decompress_append(input, output, max_size)
        } else {
            Decompressor::with_dict(dict).decompress_append(input, output, max_size)
        }
    }

    // First decompress: zero-filled buffer.
    let mut output1 = vec![0u8; buf_size];
    let result1 = decompress(input, &mut output1, &dict).map(|len| output1[..len].to_owned());

    // Uninitialized buffer. Same result, including errors.
    let mut uninit = Vec::with_capacity(buf_size);
    let result_uninit = decompress_uninit(input, &mut uninit.spare_capacity_mut()[..buf_size], &dict);
    assert_eq!(result1, result_uninit);

    // Append after existing bytes. Same result. On error, the vec is unchanged.
    let prefix = [0x55u8; 7];
    let mut appended = prefix.to_vec();
    let result_append = append(input, &mut appended, buf_size, &dict);
    match &result1 {
        Ok(result1) => {
            assert_eq!(result_append, Ok(result1.len()));
            assert_eq!(&appended[prefix.len()..], &result1[..]);
        }
        Err(err) => {
            assert_eq!(result_append.as_ref(), Err(err));
            assert_eq!(appended, prefix);
        }
    }

    let Ok(result1) = result1 else {
        return;
    };

    // Second decompress: 0xFF-filled buffer. Result must be identical.
    let mut output2 = vec![0xFFu8; buf_size];
    let len2 = decompress(input, &mut output2, &dict).unwrap();
    assert_eq!(result1, &output2[..len2]);

    // Third: 0xAA-filled. Belt and suspenders.
    let mut output3 = vec![0xAAu8; buf_size];
    let len3 = decompress(input, &mut output3, &dict).unwrap();
    assert_eq!(result1, &output3[..len3]);
});
