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

    let mut out = vec![0u8; input.len()];
    let m = lz4rip::block::decompress_into(&compressed[..n], &mut out).unwrap();
    assert_eq!(m, input.len());
    assert_eq!(out, input);
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

    let decompressor = lz4rip::block::Decompressor::with_dict(&dict);
    let out = decompressor.decompress(&compressed, input.len()).unwrap();
    assert_eq!(out, input);
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
