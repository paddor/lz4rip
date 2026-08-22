mod common;

use common::*;
use std::iter;

#[test]
fn test_end_offset() {
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAAaAAAAAAAAAAAAAAAAAAAAAAAA");
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBaAAAAAAAAAAAAAAAAAAAAAAAA");
}

#[test]
fn small_compressible_1() {
    test_roundtrip("AAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBaAAAAAAAAAAAAAAAAAAAAAAAABBBBBBBBBa");
}

#[test]
fn small_compressible_2() {
    test_roundtrip("AAAAAAAAAAAZZZZZZZZAAAAAAAA");
}

#[test]
fn small_compressible_3() {
    test_roundtrip("AAAAAAAAAAAZZZZZZZZAAAAAAAA");
}

#[test]
fn shakespear1() {
    test_roundtrip("to live or not to live");
}

#[test]
fn shakespear2() {
    test_roundtrip("Love is a wonderful terrible thing");
}

#[test]
fn shakespear3() {
    test_roundtrip("There is nothing either good or bad, but thinking makes it so.");
}

#[test]
fn shakespear4() {
    test_roundtrip("I burn, I pine, I perish.");
}

#[test]
fn text_text() {
    test_roundtrip("Save water, it doesn't grow on trees.");
    test_roundtrip("The panda bear has an amazing black-and-white fur.");
    test_roundtrip("The average panda eats as much as 9 to 14 kg of bamboo shoots a day.");
    test_roundtrip("You are 60% water. Save 60% of yourself!");
    test_roundtrip("To cute to die! Save the red panda!");
}

#[test]
fn not_compressible() {
    test_roundtrip("as6yhol.;jrew5tyuikbfewedfyjltre22459ba");
    test_roundtrip("jhflkdjshaf9p8u89ybkvjsdbfkhvg4ut08yfrr");
}

#[test]
fn short_1() {
    test_roundtrip("ahhd");
    test_roundtrip("ahd");
    test_roundtrip("x-29");
    test_roundtrip("x");
    test_roundtrip("k");
    test_roundtrip(".");
    test_roundtrip("ajsdh");
    test_roundtrip("aaaaaa");
}

#[test]
fn short_2() {
    test_roundtrip("aaaaaabcbcbcbc");
}

#[test]
fn empty_string() {
    test_roundtrip("");
}

#[test]
fn nulls() {
    test_roundtrip("\0\0\0\0\0\0\0\0\0\0\0\0\0");
}

#[test]
fn bug_fuzz() {
    let data = &[
        8, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 8, 0, 138,
    ];
    test_roundtrip(data);
}

#[test]
fn bug_fuzz_2() {
    let data = &[
        122, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 65, 0, 0, 128, 10, 1, 10, 1, 0, 122,
    ];
    test_roundtrip(data);
}

#[test]
fn bug_fuzz_3() {
    let data = &[
        36, 16, 0, 0, 79, 177, 176, 176, 171, 1, 0, 255, 207, 79, 79, 79, 79, 79, 1, 1, 49, 0, 16,
        0, 79, 79, 79, 79, 79, 1, 0, 255, 36, 79, 79, 79, 79, 79, 1, 0, 255, 207, 79, 79, 79, 79,
        79, 1, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 8, 207, 1, 207, 207, 79, 199,
        79, 79, 40, 79, 1, 1, 1, 1, 1, 1, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
        15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 79, 15, 15, 14, 15, 15, 15, 15, 15, 15,
        15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 61, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 0,
        48, 45, 0, 1, 0, 0, 1, 0,
    ];
    test_roundtrip(data);
}

#[test]
fn bug_fuzz_4() {
    let data = &[147];
    test_roundtrip(data);
}

#[test]
fn buf_fuzz_5() {
    let data = &[
        255, 255, 255, 255, 253, 235, 156, 140, 8, 0, 140, 45, 169, 0, 27, 128, 48, 0, 140, 0, 0,
        255, 255, 255, 253, 235, 156, 140, 8, 61, 255, 255, 255, 255, 65, 239, 254,
    ];
    test_roundtrip(data);
}

#[test]
fn bug_fuzz_6() {
    let data = &[
        181, 181, 181, 181, 181, 147, 147, 147, 0, 0, 255, 218, 44, 0, 177, 44, 0, 233, 177, 74,
        85, 47, 95, 146, 189, 177, 1, 0, 255, 2, 109, 180, 255, 255, 0, 0, 0, 181, 181, 181, 147,
        147, 147, 0, 0, 255, 218, 146, 146, 181, 0, 0, 181,
    ];
    test_roundtrip(data);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_so_many_zeros() {
    let data: Vec<u8> = iter::repeat_n(0, 30_000).collect();
    test_roundtrip(data);
}

#[test]
fn compression_works() {
    let s = r#"An iterator that knows its exact length.
        Many Iterators don't know how many times they will iterate, but some do. If an iterator knows how many times it can iterate, providing access to that information can be useful. For example, if you want to iterate backwards, a good start is to know where the end is.
        When implementing an ExactSizeIterator, you must also implement Iterator. When doing so, the implementation of size_hint must return the exact size of the iterator.
        The len method has a default implementation, so you usually shouldn't implement it. However, you may be able to provide a more performant implementation than the default, so overriding it in this case makes sense."#;

    test_roundtrip(s);
    assert!(lz4rip::compress(s.as_bytes()).len() < s.len());
}

#[ignore]
#[test]
fn big_compression() {
    let mut s = Vec::with_capacity(80_000_000);
    for n in 0..80_000_000 {
        s.push((n as u8).wrapping_mul(0xA).wrapping_add(33) ^ 0xA2);
    }
    test_roundtrip(s);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_dickens() {
    test_roundtrip(&*DICKENS);
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_json_66k() {
    test_roundtrip(compression66json());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_text_65k() {
    test_roundtrip(compression65());
}

#[test]
#[cfg_attr(miri, ignore)]
fn test_text_34k() {
    test_roundtrip(compression34k());
}

#[test]
fn test_text_1k() {
    test_roundtrip(compression1k());
}

use proptest::{prelude::*, test_runner::FileFailurePersistence};

fn vec_of_vec() -> impl Strategy<Value = Vec<Vec<u8>>> {
    const N: u8 = 200;
    let length = 0..N;
    length.prop_flat_map(vec_from_length)
}

fn vec_from_length(length: u8) -> impl Strategy<Value = Vec<Vec<u8>>> {
    const K: usize = u8::MAX as usize;
    let mut result = vec![];
    for index in 1..length {
        let inner = proptest::collection::vec(0..index, 0..K);
        result.push(inner);
    }
    result
}

/// CompressorRef epoch-based table reuse: compress many small inputs
/// (below EPOCH_THRESHOLD = 8 KB) through the same CompressorRef to
/// exercise stream_offset advancement and the overflow/reset path.
#[test]
fn compressor_ref_epoch_reuse() {
    use lz4rip::block::{CompressorRef, decompress, get_maximum_output_size};

    let mut comp = CompressorRef::new();
    let input = b"the quick brown fox jumps over the lazy dog, again and again!";
    let mut output = vec![0u8; get_maximum_output_size(input.len())];

    for _ in 0..70_000 {
        let n = comp.compress_into(input, &mut output).unwrap();
        let decompressed = decompress(&output[..n], input.len()).unwrap();
        assert_eq!(&decompressed, input);
    }
}

/// CompressorRef with dict: exercise the u16 boundary fallback.
/// When dict.len() + input.len() >= u16::MAX, CompressorRef::compress_into
/// falls back from compress_with_dict_table to compress_internal.
#[test]
fn compressor_ref_dict_u16_boundary_fallback() {
    use lz4rip::block::{Decompressor, DictCompressor};

    let dict = vec![b'A'; 32768];
    let input: Vec<u8> = (0u8..=255).cycle().take(40000).collect();

    let mut comp = DictCompressor::new(&dict);
    let compressed = comp.compress(&input);
    let decomp = Decompressor::with_dict(&dict);
    let decompressed = decomp.decompress(&compressed, input.len()).unwrap();
    assert_eq!(decompressed, input);
}

/// Const-generic table size: a 1 KB no-dict table (`CompressorRefN::<256>`) and
/// matching owning/dict forms still round-trip correctly. Smaller tables only
/// trade ratio for memory; they never produce incorrect output.
#[test]
fn tiny_table_roundtrip() {
    use lz4rip::block::{
        CompressorN, CompressorRefN, Decompressor, DictCompressorN, DictCompressorRefN, decompress,
        get_maximum_output_size,
    };

    let input: Vec<u8> = (0u8..=255).cycle().take(20000).collect();
    let mut output = vec![0u8; get_maximum_output_size(input.len())];

    // No-dict, borrowing, 256 entries (1 KB u32 table).
    let mut comp = CompressorRefN::<256>::new();
    let n = comp.compress_into(&input, &mut output).unwrap();
    assert_eq!(decompress(&output[..n], input.len()).unwrap(), input);

    // No-dict, owning, 512 entries.
    let mut comp = CompressorN::<512>::new();
    assert_eq!(
        decompress(&comp.compress(&input), input.len()).unwrap(),
        input
    );

    // Dict, both forms, 1024 entries (2 KB u16 tables).
    let dict = &input[..4096];
    let mut comp = DictCompressorRefN::<1024>::new(dict);
    let n = comp.compress_into(&input, &mut output).unwrap();
    let decomp = Decompressor::with_dict(dict);
    assert_eq!(decomp.decompress(&output[..n], input.len()).unwrap(), input);

    let mut comp = DictCompressorN::<1024>::new(dict);
    let compressed = comp.compress(&input);
    assert_eq!(decomp.decompress(&compressed, input.len()).unwrap(), input);

    // Dict + input >= 64 KB forces the u16->u32 overflow fallback, which now
    // builds a `HashTableU32<N>` sized to this compressor's N (256), not a
    // default 8 KB table.
    let big: Vec<u8> = (0u8..=255).cycle().take(70_000).collect();
    let big_dict = vec![b'Z'; 40_000];
    let mut out2 = vec![0u8; get_maximum_output_size(big.len())];
    let mut comp = DictCompressorRefN::<256>::new(&big_dict);
    let n = comp.compress_into(&big, &mut out2).unwrap();
    let decomp = Decompressor::with_dict(&big_dict);
    assert_eq!(decomp.decompress(&out2[..n], big.len()).unwrap(), big);
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("regressions"))),
        ..Default::default()
    })]

    #[test]
    #[cfg_attr(miri, ignore)]
    fn proptest_roundtrip(v in vec_of_vec()) {
        let data: Vec<u8> = v.iter().flat_map(|v| v.iter()).cloned().collect::<Vec<_>>();
        test_roundtrip(data);
    }
}
