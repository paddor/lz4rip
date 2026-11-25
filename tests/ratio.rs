//! Ratio profile: `compress_ratio`, `RatioCompressor`, and
//! `FrameEncoder::ratio_profile`.

use lz4rip::block::{
    RatioCompressor, compress_into, compress_ratio, compress_ratio_into, decompress,
    get_maximum_output_size,
};

/// Pseudo-random words from a skewed vocabulary: text-like data where a
/// denser search finds more matches than the default one.
fn words(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    let vocab: Vec<Vec<u8>> = (0..2000)
        .map(|_| {
            let n = 2 + next() % 9;
            (0..n).map(|_| b'a' + (next() % 26) as u8).collect()
        })
        .collect();
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        // Squaring skews picks toward the front of the vocabulary.
        let r = next() % vocab.len();
        out.extend_from_slice(&vocab[r * r / vocab.len()]);
        out.push(if next() % 12 == 0 { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

const SIZES: &[usize] = &[
    0, 1, 12, 13, 100, 4095, 4096, 4097, 8192, 65534, 65535, 65536, 65537, 200_000, 600_000,
];

#[test]
fn block_roundtrip_and_compressor_matches_one_shot() {
    let mut comp = RatioCompressor::new();
    for &len in SIZES {
        for input in [words(len, len as u64), noise(len, len as u64 + 1)] {
            let one_shot = compress_ratio(&input);
            assert_eq!(decompress(&one_shot, len).unwrap(), input, "len {len}");

            let mut out = vec![0u8; get_maximum_output_size(len)];
            let n = compress_ratio_into(&input, &mut out).unwrap();
            assert_eq!(&out[..n], &one_shot[..], "into, len {len}");

            // Reused across sizes.
            assert_eq!(comp.compress(&input), one_shot, "compressor, len {len}");
        }
    }
}

#[test]
fn compressor_reuse_matches_one_shot() {
    // Inputs up to 256 B reuse the small table until `u16` positions would
    // wrap. The first 900 calls (142 KB) wrap it twice. After them, 257 B to
    // 4 KiB inputs clear it in between, and larger inputs use the other table.
    let mut comp = RatioCompressor::new();
    for i in 0..1200u64 {
        let len = if i < 900 {
            [13, 64, 200, 256, 255][i as usize % 5]
        } else {
            match i % 8 {
                0 => 4096,
                1 => 13,
                2 => 256,
                3 => 257,
                4 => 1000,
                5 => 64,
                6 if i % 3 == 0 => 20_000,
                _ => 3000,
            }
        };
        let input = if i % 7 == 0 {
            noise(len, i)
        } else {
            words(len, i)
        };
        let one_shot = compress_ratio(&input);
        assert_eq!(comp.compress(&input), one_shot, "call {i}, len {len}");
        assert_eq!(decompress(&one_shot, len).unwrap(), input, "call {i}");
    }
}

#[test]
fn output_too_small_is_an_error() {
    let input = words(10_000, 7);
    let mut out = vec![0u8; get_maximum_output_size(input.len()) - 1];
    assert!(compress_ratio_into(&input, &mut out).is_err());
    assert!(
        RatioCompressor::new()
            .compress_into(&input, &mut out)
            .is_err()
    );
}

#[test]
fn smaller_than_default_on_text() {
    for len in [8 * 1024, 64 * 1024, 512 * 1024] {
        let input = words(len, 42);
        let mut out = vec![0u8; get_maximum_output_size(len)];
        let fast = compress_into(&input, &mut out).unwrap();
        let ratio = compress_ratio(&input).len();
        assert!(ratio < fast, "len {len}: ratio {ratio} >= default {fast}");
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "gnu")))]
#[test]
fn c_lz4_decodes_ratio_blocks() {
    for &len in SIZES {
        let input = words(len, len as u64 + 3);
        let compressed = compress_ratio(&input);
        let mut out = vec![0u8; len];
        let n = lzzzz::lz4::decompress(&compressed, &mut out).unwrap();
        assert_eq!(n, len);
        assert_eq!(out, input, "len {len}");
    }
}

#[cfg(feature = "frame")]
mod frame {
    use super::{noise, words};
    use lz4rip::frame::{BlockMode, BlockSize, FrameDecoder, FrameEncoder, FrameInfo};
    use std::io::{Read, Write};

    fn encode(enc: FrameEncoder<Vec<u8>>, input: &[u8], chunk: usize) -> Vec<u8> {
        let mut enc = enc.ratio_profile();
        for c in input.chunks(chunk.max(1)) {
            enc.write_all(c).unwrap();
        }
        enc.finish().unwrap()
    }

    fn decode(mut dec: FrameDecoder<&[u8]>) -> Vec<u8> {
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();
        out
    }

    fn infos() -> Vec<FrameInfo> {
        let mut v = Vec::new();
        for mode in [BlockMode::Independent, BlockMode::Linked] {
            for size in [BlockSize::Max64KB, BlockSize::Max256KB] {
                v.push(
                    FrameInfo::new()
                        .block_mode(mode)
                        .block_size(size)
                        .content_checksum(true),
                );
            }
        }
        v
    }

    #[test]
    fn roundtrip() {
        for info in infos() {
            for len in [0, 1000, 70_000, 300_000] {
                for input in [words(len, 9), noise(len, 10)] {
                    for chunk in [7_000, 100_000] {
                        let enc = FrameEncoder::with_frame_info(info, Vec::new());
                        let compressed = encode(enc, &input, chunk);
                        assert_eq!(decode(FrameDecoder::new(&compressed)), input);
                    }
                }
            }
        }
    }

    #[test]
    fn dictionary_roundtrip() {
        let dict = words(20_000, 11);
        for info in infos() {
            for len in [0, 1000, 300_000] {
                let input = words(len, 12);
                let enc = FrameEncoder::with_dictionary(Vec::new(), &dict, 5, Some(info)).unwrap();
                let compressed = encode(enc, &input, 50_000);
                let dec = FrameDecoder::with_dictionary(&compressed[..], &dict, 5);
                assert_eq!(decode(dec), input);
            }
        }
    }

    #[test]
    fn smaller_than_default() {
        let input = words(1 << 20, 13);
        for info in infos() {
            let mut fast = FrameEncoder::with_frame_info(info, Vec::new());
            fast.write_all(&input).unwrap();
            let fast = fast.finish().unwrap().len();
            let enc = FrameEncoder::with_frame_info(info, Vec::new());
            let ratio = encode(enc, &input, input.len()).len();
            assert!(ratio < fast, "{info:?}: ratio {ratio} >= default {fast}");
        }
    }

    #[cfg(not(all(target_arch = "arm", target_os = "linux", target_env = "gnu")))]
    #[test]
    fn c_lz4_decodes_ratio_frames() {
        let input = words(300_000, 14);
        for info in infos() {
            let enc = FrameEncoder::with_frame_info(info, Vec::new());
            let compressed = encode(enc, &input, 70_000);
            let mut out = Vec::new();
            lzzzz::lz4f::decompress_to_vec(&compressed, &mut out).unwrap();
            assert_eq!(out, input);
        }
    }
}
