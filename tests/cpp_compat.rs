#![cfg(not(miri))]
#![cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "gnu")))]

mod common;

use common::*;
#[cfg(feature = "frame")]
use lz4rip::frame::BlockMode;
use lz4rip::{block::decompress, compress as compress_block};

fn lz4_cpp_block_compress(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    lzzzz::lz4::compress_to_vec(input, &mut out, lzzzz::lz4::ACC_LEVEL_DEFAULT).unwrap();
    out
}

fn lz4_cpp_block_decompress(input: &[u8], decomp_len: usize) -> Vec<u8> {
    let mut out = vec![0u8; decomp_len];
    lzzzz::lz4::decompress(input, &mut out).unwrap();
    out
}

#[cfg(feature = "frame")]
fn lz4_cpp_frame_compress(input: &[u8], independent: bool) -> Vec<u8> {
    let pref = lzzzz::lz4f::PreferencesBuilder::new()
        .block_mode(if independent {
            lzzzz::lz4f::BlockMode::Independent
        } else {
            lzzzz::lz4f::BlockMode::Linked
        })
        .build();
    let mut out = Vec::new();
    lzzzz::lz4f::compress_to_vec(input, &mut out, &pref).unwrap();
    out
}

#[cfg(feature = "frame")]
fn lz4_cpp_frame_decompress(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    lzzzz::lz4f::decompress_to_vec(input, &mut out).unwrap();
    out
}

fn test_block_compat(bytes: &[u8]) {
    if !bytes.is_empty() {
        let compressed = lz4_cpp_block_compress(bytes);
        let decompressed = decompress(&compressed, bytes.len()).unwrap();
        assert_eq!(decompressed, bytes);
    }

    let compressed = compress_block(bytes);
    let decompressed = lz4_cpp_block_decompress(&compressed, bytes.len());
    assert_eq!(decompressed, bytes);
}

#[cfg(feature = "frame")]
fn test_frame_compat(bytes: &[u8]) {
    // C compress -> Rust decompress
    for independent in [true, false] {
        let compressed = lz4_cpp_frame_compress(bytes, independent);
        let decompressed = lz4rip_frame_decompress(&compressed).unwrap();
        assert_eq!(decompressed, bytes);
    }

    // Rust compress -> C decompress
    for bm in &[BlockMode::Independent, BlockMode::Linked] {
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.block_mode = *bm;
        let compressed = lz4rip_frame_compress_with(frame_info, bytes).unwrap();
        let decompressed = lz4_cpp_frame_decompress(&compressed);
        assert_eq!(decompressed, bytes);
    }
}

#[test]
fn block_compat_1k() {
    test_block_compat(compression1k());
}

#[test]
fn block_compat_34k() {
    test_block_compat(compression34k());
}

#[test]
fn block_compat_65k() {
    test_block_compat(compression65());
}

#[test]
fn block_compat_66k_json() {
    test_block_compat(compression66json());
}

#[test]
fn block_compat_dickens() {
    test_block_compat(&DICKENS);
}

#[test]
fn block_compat_empty() {
    test_block_compat(b"");
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_1k() {
    test_frame_compat(compression1k());
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_34k() {
    test_frame_compat(compression34k());
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_65k() {
    test_frame_compat(compression65());
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_66k_json() {
    test_frame_compat(compression66json());
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_dickens() {
    test_frame_compat(&DICKENS);
}

#[cfg(feature = "frame")]
#[test]
fn frame_compat_empty() {
    test_frame_compat(b"");
}

#[test]
fn compare_compression() {
    fn print_compression_ratio(input: &[u8], name: &str) {
        println!("\nComparing for {name}");

        let compressed = compress_block(input);
        println!(
            "lz4rip block Compression Ratio {:.4}",
            compressed.len() as f64 / input.len() as f64
        );
        let decompressed = decompress(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);

        let compressed = lz4_cpp_block_compress(input);
        println!(
            "Lz4 Cpp block Compression Ratio {:.4}",
            compressed.len() as f64 / input.len() as f64
        );
        let decompressed = decompress(&compressed, input.len()).unwrap();
        assert_eq!(decompressed, input);

        let compressed = snap::raw::Encoder::new().compress_vec(input).unwrap();
        println!(
            "snap Compression Ratio {:.4}",
            compressed.len() as f64 / input.len() as f64
        );

        #[cfg(feature = "frame")]
        {
            let mut frame_info = lz4rip::frame::FrameInfo::new();
            frame_info.block_mode = BlockMode::Independent;
            let compressed = lz4rip_frame_compress_with(frame_info, input).unwrap();
            println!(
                "lz4rip frame indep Compression Ratio {:.4}",
                compressed.len() as f64 / input.len() as f64
            );

            let mut frame_info = lz4rip::frame::FrameInfo::new();
            frame_info.block_mode = BlockMode::Linked;
            let compressed = lz4rip_frame_compress_with(frame_info, input).unwrap();
            println!(
                "lz4rip frame linked Compression Ratio {:.4}",
                compressed.len() as f64 / input.len() as f64
            );

            let compressed = lz4_cpp_frame_compress(input, true);
            println!(
                "lz4 cpp frame indep Compression Ratio {:.4}",
                compressed.len() as f64 / input.len() as f64
            );

            let compressed = lz4_cpp_frame_compress(input, false);
            println!(
                "lz4 cpp frame linked Compression Ratio {:.4}",
                compressed.len() as f64 / input.len() as f64
            );
        }
    }

    print_compression_ratio(compression1k(), "1k");
    print_compression_ratio(compression34k(), "34k");
    print_compression_ratio(compression66json(), "66k JSON");
    print_compression_ratio(&DICKENS, "dickens");
}

#[test]
fn test_comp_lz4_linked() {
    fn print_ratio(text: &str, val1: usize, val2: usize) {
        println!(
            "{:?} {:.3} {} -> {}",
            text,
            val1 as f32 / val2 as f32,
            val1,
            val2
        );
    }

    fn get_compressed_size(input: &[u8]) -> usize {
        lz4_cpp_block_compress(input).len()
    }

    print_ratio(
        "Ratio 1k C",
        compression1k().len(),
        get_compressed_size(compression1k()),
    );
    print_ratio(
        "Ratio 34k C",
        compression34k().len(),
        get_compressed_size(compression34k()),
    );
}
