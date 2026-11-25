#[cfg(feature = "frame")]
use std::io::{Read, Write};

fn sample(i: usize) -> Vec<u8> {
    format!(
        r#"{{"ts":"2026-07-19T12:00:{:02}Z","service":"omq","route":"worker-{}","status":200,"payload":"{}"}}"#,
        i % 60,
        i % 17,
        "abcdef0123456789".repeat(8)
    )
    .into_bytes()
}

#[test]
fn block_roundtrip_no_dict() {
    let input: Vec<u8> = (0u8..=255).cycle().take(128 * 1024).collect();
    let mut compressed = vec![0u8; lz4rip::block::get_maximum_output_size(input.len())];
    let n = lz4rip::block::compress_into(&input, &mut compressed).unwrap();
    let validation = lz4rip::block::validate_block(&compressed[..n], input.len()).unwrap();
    assert_eq!(validation.decoded_len, input.len());

    let mut out = vec![0u8; input.len()];
    let m = lz4rip::block::decompress_into(&compressed[..n], &mut out).unwrap();
    assert_eq!(m, input.len());
    assert_eq!(out, input);

    let mut out = b"prefix".to_vec();
    let m = lz4rip::block::decompress_append(&compressed[..n], &mut out, input.len()).unwrap();
    assert_eq!(m, input.len());
    assert_eq!(&out[6..], input);
}

#[test]
fn block_roundtrip_with_trained_dict() {
    let mut trainer = lz4rip::block::DictTrainer::new(4096);
    for i in 0..64 {
        trainer.add_sample(&sample(i));
    }
    let dict = trainer.train();
    assert!(!dict.is_empty());

    let input = sample(999);
    let mut compressor = lz4rip::block::DictCompressor::new(&dict);
    let compressed = compressor.compress(&input);
    let validation =
        lz4rip::block::validate_block_with_dict(&compressed, input.len(), &dict).unwrap();
    assert_eq!(validation.decoded_len, input.len());

    let decompressor = lz4rip::block::Decompressor::with_dict(&dict);
    let out = decompressor.decompress(&compressed, input.len()).unwrap();
    assert_eq!(out, input);
}

#[test]
fn validator_checks_length_arithmetic() {
    let block = [0xF0, 0xFF];
    assert!(matches!(
        lz4rip::block::validate_block(&block, usize::MAX),
        Err(lz4rip::block::DecompressError::ExpectedAnotherByte)
    ));

    assert!(matches!(
        lz4rip::block::validate_block(&block, 1),
        Err(lz4rip::block::DecompressError::DecodedSizeMismatch { .. })
    ));
}

#[cfg(feature = "frame")]
#[test]
fn frame_roundtrip_linked_blocks() {
    let mut frame_info = lz4rip::frame::FrameInfo::new();
    frame_info.block_mode = lz4rip::frame::BlockMode::Linked;

    let mut compressed = Vec::new();
    {
        let mut encoder = lz4rip::frame::FrameEncoder::with_frame_info(frame_info, &mut compressed);
        for i in 0..256 {
            encoder.write_all(&sample(i)).unwrap();
        }
        encoder.finish().unwrap();
    }

    let mut decoder = lz4rip::frame::FrameDecoder::new(compressed.as_slice());
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).unwrap();

    let mut expected = Vec::new();
    for i in 0..256 {
        expected.extend_from_slice(&sample(i));
    }
    assert_eq!(out, expected);
}

/// Word salad from a fixed seed: compressible, with enough distinct 5-byte
/// sequences that a different hash changes the output.
fn word_salad(len: usize, mut seed: u64) -> Vec<u8> {
    let words: Vec<&str> = "the quick brown fox jumps over lazy dog socket message frame \
        block worker route status payload dictionary stream offset literal match token \
        buffer window header checksum"
        .split_whitespace()
        .collect();
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.extend_from_slice(words[(seed % words.len() as u64) as usize].as_bytes());
        if seed.is_multiple_of(7) {
            out.extend_from_slice(format!("{}", (seed >> 8) % 1000).as_bytes());
        }
        out.push(if seed.is_multiple_of(11) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

fn digest(data: &[u8]) -> (usize, u64) {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    (data.len(), h)
}

/// Compressed bytes are the same on every target (pointer width, endianness).
#[test]
fn compressed_output_is_target_independent() {
    let mut actual: Vec<(&str, (usize, u64))> = Vec::new();

    let small = word_salad(4 * 1024, 1);
    actual.push(("compress 4K", digest(&lz4rip::block::compress(&small))));

    let large = word_salad(96 * 1024, 2);
    actual.push(("compress 96K", digest(&lz4rip::block::compress(&large))));

    for (name, len, seed) in [
        ("compress_ratio 4K", 4 * 1024, 8),
        ("compress_ratio 32K", 32 * 1024, 9),
        ("compress_ratio 96K", 96 * 1024, 10),
    ] {
        let input = word_salad(len, seed);
        actual.push((name, digest(&lz4rip::block::compress_ratio(&input))));
    }

    let msgs = word_salad(8 * 1024, 3);
    let mut compressor = lz4rip::block::Compressor::new();
    let mut out = Vec::new();
    for m in msgs.chunks(1024) {
        out.extend_from_slice(&compressor.compress(m));
    }
    actual.push(("Compressor 8x1K", digest(&out)));

    let mut trainer = lz4rip::block::DictTrainer::new(2048);
    for s in word_salad(64 * 1024, 4).chunks(512) {
        trainer.add_sample(s);
    }
    let dict = trainer.train();
    actual.push(("dict", digest(&dict)));
    let mut compressor = lz4rip::block::DictCompressor::new(&dict);
    let mut out = Vec::new();
    for m in word_salad(4 * 512, 5).chunks(512) {
        out.extend_from_slice(&compressor.compress(m));
    }
    out.extend_from_slice(&compressor.compress(&word_salad(70 * 1024, 6)));
    actual.push(("DictCompressor", digest(&out)));

    #[cfg(feature = "frame")]
    {
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.block_mode = lz4rip::frame::BlockMode::Linked;
        let mut out = Vec::new();
        let mut encoder = lz4rip::frame::FrameEncoder::with_frame_info(frame_info, &mut out);
        encoder.write_all(&word_salad(160 * 1024, 7)).unwrap();
        encoder.finish().unwrap();
        actual.push(("frame linked 160K", digest(&out)));

        let mut out = Vec::new();
        let mut encoder =
            lz4rip::frame::FrameEncoder::with_frame_info(frame_info, &mut out).ratio_profile();
        encoder.write_all(&word_salad(160 * 1024, 7)).unwrap();
        encoder.finish().unwrap();
        actual.push(("ratio frame linked 160K", digest(&out)));
    }

    let expected: &[(&str, (usize, u64))] = &[
        ("compress 4K", (2083, 0xa77a_38b6_fd13_5474)),
        ("compress 96K", (45256, 0x7740_2478_7216_398b)),
        ("compress_ratio 4K", (2090, 0x6893_bc7d_3ffc_f386)),
        ("compress_ratio 32K", (15011, 0xbf7e_fca4_6a54_076b)),
        ("compress_ratio 96K", (43868, 0xbbbf_dc65_7183_c9c8)),
        ("Compressor 8x1K", (5062, 0x034f_e6ef_6b8c_ea2a)),
        ("dict", (2048, 0x490c_2e96_5d2b_e7bf)),
        ("DictCompressor", (33351, 0x869e_798e_207c_12f1)),
        ("frame linked 160K", (74618, 0x5235_69e2_f3e5_810f)),
        ("ratio frame linked 160K", (72649, 0xd62c_c626_c908_42ca)),
    ];
    for (name, got) in &actual {
        let (_, want) = expected.iter().find(|(n, _)| n == name).unwrap();
        assert_eq!(got, want, "{name}");
    }
}
