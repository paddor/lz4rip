//! Every block compressor fits its output in `get_maximum_output_size`
//! bytes, C lz4's `LZ4_compressBound`, including incompressible input, the
//! worst case. Vec results keep little spare capacity.

use lz4rip::block::{
    CompressorRef, DictCompressorRef, compress_into, compress_into_with_dict, decompress_into,
    decompress_into_with_dict, get_maximum_output_size,
};

/// xorshift bytes: no 4-byte repeats to speak of, so all literals.
fn random(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect()
}

/// Random runs with short repeats, so sequences of every kind.
fn mixed(len: usize, seed: u64) -> Vec<u8> {
    let noise = random(len, seed);
    let mut out = Vec::with_capacity(len);
    let mut i = 0;
    while out.len() < len {
        let run = 1 + noise[i % len] as usize % 300;
        out.extend_from_slice(&noise[i % len..(i % len + run).min(len)]);
        if noise[(i + 1) % len] & 1 == 0 && out.len() > 8 {
            let back = out.len() - 1 - noise[(i + 2) % len] as usize % (out.len() - 1).min(255);
            out.extend_from_within(back..back + 4.min(out.len() - back));
        }
        i += run + 3;
    }
    out.truncate(len);
    out
}

fn sizes() -> impl Iterator<Item = usize> {
    let edges = (1..=40).flat_map(|k| [255 * k - 1, 255 * k, 255 * k + 1]);
    let large = [8191, 8192, 65_534, 65_535, 65_536, 65_537, 131_072, 300_000];
    (0..=4096).chain(edges).chain(large)
}

/// Compresses `input` into a buffer of exactly the bound with `compress`,
/// and checks the output decodes back.
fn check(
    name: &str,
    input: &[u8],
    compress: impl FnOnce(&[u8], &mut [u8]) -> usize,
    decode: impl FnOnce(&[u8], &mut [u8]) -> usize,
) {
    let mut out = vec![0u8; get_maximum_output_size(input.len())];
    let len = compress(input, &mut out);
    let mut back = vec![0u8; input.len()];
    assert_eq!(
        decode(&out[..len], &mut back),
        input.len(),
        "{name}, len {}",
        input.len()
    );
    assert_eq!(back, input, "{name}, len {}", input.len());
}

#[test]
fn bound_fits_every_block_compressor() {
    let dict = mixed(2_000, 99);
    let mut reused = CompressorRef::new();
    let mut dict_reused = DictCompressorRef::new(&dict);
    for len in sizes() {
        for input in [random(len, len as u64 + 1), mixed(len, len as u64 + 7)] {
            let plain = |c: &[u8], b: &mut [u8]| decompress_into(c, b).unwrap();
            let with_dict =
                |c: &[u8], b: &mut [u8]| decompress_into_with_dict(c, b, &dict).unwrap();
            check(
                "compress_into",
                &input,
                |i, o| compress_into(i, o).unwrap(),
                plain,
            );
            check(
                "compress_into_with_dict",
                &input,
                |i, o| compress_into_with_dict(i, o, &dict).unwrap(),
                with_dict,
            );
            check(
                "CompressorRef",
                &input,
                |i, o| reused.compress_into(i, o).unwrap(),
                plain,
            );
            check(
                "DictCompressorRef",
                &input,
                |i, o| dict_reused.compress_into(i, o).unwrap(),
                with_dict,
            );
            #[cfg(feature = "alloc")]
            check(
                "compress_ratio_into",
                &input,
                |i, o| lz4rip::block::compress_ratio_into(i, o).unwrap(),
                plain,
            );
        }
    }
}

#[cfg(feature = "alloc")]
#[test]
fn bound_fits_owning_compressors() {
    use lz4rip::block::{Compressor, DictCompressor, RatioCompressor};
    let dict = mixed(2_000, 99);
    let (mut plain_c, mut dict_c, mut ratio_c) = (
        Compressor::new(),
        DictCompressor::new(&dict),
        RatioCompressor::new(),
    );
    for len in sizes() {
        for input in [random(len, len as u64 + 3), mixed(len, len as u64 + 5)] {
            let plain = |c: &[u8], b: &mut [u8]| decompress_into(c, b).unwrap();
            let with_dict =
                |c: &[u8], b: &mut [u8]| decompress_into_with_dict(c, b, &dict).unwrap();
            check(
                "Compressor",
                &input,
                |i, o| plain_c.compress_into(i, o).unwrap(),
                plain,
            );
            check(
                "DictCompressor",
                &input,
                |i, o| dict_c.compress_into(i, o).unwrap(),
                with_dict,
            );
            check(
                "RatioCompressor",
                &input,
                |i, o| ratio_c.compress_into(i, o).unwrap(),
                plain,
            );
        }
    }
}

#[test]
fn one_byte_less_than_the_bound_is_rejected() {
    let input = random(10_000, 1);
    let mut out = vec![0u8; get_maximum_output_size(input.len()) - 1];
    assert!(compress_into(&input, &mut out).is_err());
}

#[cfg(feature = "alloc")]
#[test]
fn vec_results_keep_little_spare_capacity() {
    use lz4rip::block::{
        Compressor, Decompressor, DictCompressor, RatioCompressor, compress, compress_ratio,
        decompress,
    };
    const SPARE: usize = 4 * 1024;
    // Compresses about 30x, so the worst-case allocation is mostly spare.
    let input: Vec<u8> = (0..300_000u32)
        .flat_map(|i| format!("key={} ", i % 50).into_bytes())
        .take(300_000)
        .collect();
    let dict = input[..2_000].to_vec();
    let results = [
        ("compress", compress(&input)),
        ("compress_ratio", compress_ratio(&input)),
        ("Compressor", Compressor::new().compress(&input)),
        (
            "DictCompressor",
            DictCompressor::new(&dict).compress(&input),
        ),
        ("RatioCompressor", RatioCompressor::new().compress(&input)),
    ];
    for (name, out) in &results {
        assert!(out.len() < input.len() / 10, "{name}: {} bytes", out.len());
        assert!(
            out.capacity() - out.len() < SPARE,
            "{name}: capacity {}",
            out.capacity()
        );
    }
    // A loose size bound, as a caller with a maximum message size passes.
    let compressed = &results[0].1;
    for out in [
        decompress(compressed, 1 << 20).unwrap(),
        Decompressor::new().decompress(compressed, 1 << 20).unwrap(),
    ] {
        assert_eq!(out, input);
        assert!(
            out.capacity() - out.len() < SPARE,
            "capacity {}",
            out.capacity()
        );
    }
    // Small results keep their capacity: no reallocation per small message.
    let small = compress(&input[..1_000]);
    assert_eq!(small.capacity(), get_maximum_output_size(1_000));
}
