#![no_main]
use libfuzzer_sys::fuzz_target;

use lz4rip::block::{
    decompress_into, get_maximum_output_size, CompressorRefN, Decompressor, DictCompressorRefN,
};

/// Round-trip through the const-generic small-table compressors at the minimum
/// and intermediate entry counts:
/// - `CompressorRefN<256>` / `<512>`: no-dict, smallest u32 tables (epoch reuse).
/// - `DictCompressorRefN<256>` / `<1024>`: dict path, dual u16 tables, plus the
///   u16-overflow fallback (dict + input >= 64 KB) which builds a fresh
///   `HashTableU32<N>` sized to the compressor's own tiny `N`.
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

    let max_out = get_maximum_output_size(payload.len());
    let mut comp_buf = vec![0u8; max_out];
    let mut decomp_buf = vec![0u8; payload.len()];

    // No-dict, smallest and next-up u32 tables.
    {
        let mut comp = CompressorRefN::<256>::new();
        let n = comp.compress_into(&payload, &mut comp_buf).unwrap();
        let m = decompress_into(&comp_buf[..n], &mut decomp_buf).unwrap();
        assert_eq!(&payload[..], &decomp_buf[..m]);
    }
    {
        let mut comp = CompressorRefN::<512>::new();
        let n = comp.compress_into(&payload, &mut comp_buf).unwrap();
        let m = decompress_into(&comp_buf[..n], &mut decomp_buf).unwrap();
        assert_eq!(&payload[..], &decomp_buf[..m]);
    }

    // Dict path with tiny tables, including the >= 64 KB overflow fallback.
    let dict_len = input.dict.len().min(128 * 1024);
    let dict = &input.dict[..dict_len];
    let decomp = Decompressor::with_dict(dict);
    {
        let mut comp = DictCompressorRefN::<256>::new(dict);
        let n = comp.compress_into(&payload, &mut comp_buf).unwrap();
        let out = decomp.decompress(&comp_buf[..n], payload.len()).unwrap();
        assert_eq!(payload, out);
    }
    {
        let mut comp = DictCompressorRefN::<1024>::new(dict);
        let n = comp.compress_into(&payload, &mut comp_buf).unwrap();
        let out = decomp.decompress(&comp_buf[..n], payload.len()).unwrap();
        assert_eq!(payload, out);
    }
});
