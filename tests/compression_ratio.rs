mod common;

use common::*;
use lz4rip::compress as compress_block;
use more_asserts::assert_lt;

#[test]
fn test_minimum_compression_ratio_block() {
    let compressed = compress_block(compression34k());
    let ratio = compressed.len() as f64 / compression34k().len() as f64;
    assert_lt!(ratio, 0.59);

    let compressed = compress_block(compression65());
    let ratio = compressed.len() as f64 / compression65().len() as f64;
    assert_lt!(ratio, 0.59);

    let compressed = compress_block(compression66json());
    let ratio = compressed.len() as f64 / compression66json().len() as f64;
    assert_lt!(ratio, 0.240);
}

#[cfg(feature = "frame")]
#[test]
fn test_minimum_compression_ratio_frame() {
    use lz4rip::frame::FrameInfo;

    let get_ratio = |input| {
        let compressed = lz4rip_frame_compress_with(FrameInfo::new(), input).unwrap();
        compressed.len() as f64 / input.len() as f64
    };

    let ratio = get_ratio(compression34k());
    assert_lt!(ratio, 0.645);

    let ratio = get_ratio(compression65());
    assert_lt!(ratio, 0.645);

    let ratio = get_ratio(compression66json());
    assert_lt!(ratio, 0.245);
}

fn print_ratio(text: &str, val1: usize, val2: usize) {
    println!(
        "{:?} {:.3} {} -> {}",
        text,
        val1 as f32 / val2 as f32,
        val1,
        val2
    );
}

#[test]
fn test_comp_flex() {
    print_ratio(
        "Ratio 1k flex",
        compression1k().len(),
        compress_block(compression1k()).len(),
    );
    print_ratio(
        "Ratio 34k flex",
        compression34k().len(),
        compress_block(compression34k()).len(),
    );
}

/// Text from a 64-word vocabulary: repeats are short and sparse, like prose.
fn wordy_text(len: usize, seed: u64) -> Vec<u8> {
    const WORDS: &str = "the of and to in is was for on that with as by at from his her \
        they this have had were which their are but not one all been when there she \
        would what so if will more no out up into could them than then some other \
        time very about only upon over such said great before after little";
    let words: Vec<&str> = WORDS.split_whitespace().collect();
    let mut x = seed;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(words[(x >> 58) as usize % words.len()].as_bytes());
        out.push(b' ');
    }
    out.truncate(len);
    out
}

/// Small messages still compress usefully with throughput-first skipping.
/// Rotate inputs and cross the table-reuse boundary with one compressor.
#[test]
fn small_text_compresses_with_reused_compressor() {
    let mut compressor = lz4rip::block::Compressor::new();
    let cases = [
        (1023, 1.25),
        (1024, 1.25),
        (1025, 1.25),
        (2047, 1.4),
        (2048, 1.4),
        (2049, 1.4),
    ];
    let mut total_bytes = [0; 6];
    for seed in 1..=64 {
        for (i, &(len, _)) in cases.iter().enumerate() {
            let data = wordy_text(len, 0x9E37_79B9_7F4A_7C15 ^ seed);
            let compressed = compressor.compress(&data);
            total_bytes[i] += compressed.len();
            assert_eq!(lz4rip::decompress(&compressed, len).unwrap(), data);
        }
    }
    for ((len, min_ratio), compressed) in cases.into_iter().zip(total_bytes) {
        let ratio = (64 * len) as f64 / compressed as f64;
        assert!(ratio > min_ratio, "{len} bytes: aggregate ratio {ratio:.3}");
    }
}
