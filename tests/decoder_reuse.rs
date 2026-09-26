#![cfg(feature = "frame")]
//! A reused decoder must decode every input exactly like a fresh decoder,
//! even after decoding other, possibly malformed, inputs.
//!
//! `FrameDecoder` is reused by replacing its reader through `get_mut`.

use std::io::{Read, Write};

use lz4rip::block::{DecompressError, Decompressor, DecompressorRef};
use lz4rip::frame::{
    BlockMode, BlockSize, Error, FrameDecoder, FrameDecoderOptions, FrameEncoder, FrameInfo,
};

const DICT_ID: u32 = 0x0D1C_7001;
const BLOCK: usize = 64 * 1024;
const END_MARK: [u8; 4] = [0; 4];
/// Compressed block whose first match reaches 65535 bytes before the block
/// start, past any dictionary used here.
const FAR_MATCH_BLOCK: [u8; 5] = [0x00, 0xFF, 0xFF, 0x10, b'z'];

fn dict() -> Vec<u8> {
    b"level=INFO service=ingest event= message=decoder reuse ".repeat(8)
}

fn text(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(
            format!("level=INFO service=ingest event={i} message=reuse\n").as_bytes(),
        );
        i += 1;
    }
    out.truncate(len);
    out
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// Five 64 KB blocks of text, noise, and text again. The noise block is
/// stored uncompressed. Linked frames wrap their history window on the
/// fourth block.
fn multi_block_payload() -> Vec<u8> {
    let mut data = text(BLOCK);
    data.extend(noise(BLOCK, 7));
    data.extend(text(3 * BLOCK - 1000));
    data
}

fn encode(info: FrameInfo, dict: Option<&[u8]>, data: &[u8]) -> Vec<u8> {
    let mut enc = match dict {
        Some(dict) => FrameEncoder::with_dictionary(Vec::new(), dict, DICT_ID, Some(info)).unwrap(),
        None => FrameEncoder::with_frame_info(info, Vec::new()),
    };
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Frame header of `info`, taken from an empty frame.
fn header(info: FrameInfo, dict: Option<&[u8]>) -> Vec<u8> {
    let frame = encode(info, dict, b"");
    let trailer = END_MARK.len() + if info.content_checksum { 4 } else { 0 };
    frame[..frame.len() - trailer].to_vec()
}

fn uncompressed_block(data: &[u8]) -> Vec<u8> {
    let mut block = (data.len() as u32 | 0x8000_0000).to_le_bytes().to_vec();
    block.extend_from_slice(data);
    block
}

fn compressed_block(data: &[u8]) -> Vec<u8> {
    let mut block = (data.len() as u32).to_le_bytes().to_vec();
    block.extend_from_slice(data);
    block
}

/// Offsets of the data block headers in `frame`.
fn block_offsets(frame: &[u8], header_len: usize, block_checksums: bool) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut pos = header_len;
    loop {
        let size = u32::from_le_bytes(frame[pos..pos + 4].try_into().unwrap());
        if size == 0 {
            return offsets;
        }
        offsets.push(pos);
        pos += 4 + (size & 0x7FFF_FFFF) as usize + if block_checksums { 4 } else { 0 };
    }
}

type Outcome = (Vec<u8>, Result<(), String>);

fn new_decoder<'a>(input: &'a [u8], dict: Option<&[u8]>) -> FrameDecoder<&'a [u8]> {
    match dict {
        Some(dict) => FrameDecoder::with_dictionary(input, dict, DICT_ID),
        None => FrameDecoder::new(input),
    }
}

fn read_all(decoder: &mut FrameDecoder<&[u8]>) -> Outcome {
    let mut output = Vec::new();
    let result = decoder
        .read_to_end(&mut output)
        .map(drop)
        .map_err(|e| e.to_string());
    (output, result)
}

fn decode_fresh(input: &[u8], dict: Option<&[u8]>) -> Outcome {
    read_all(&mut new_decoder(input, dict))
}

/// Decodes `inputs` in order with one reused decoder and asserts that every
/// outcome, including partial output before an error, equals a fresh
/// decoder's outcome.
fn assert_reuse_matches_fresh(inputs: &[&[u8]], dict: Option<&[u8]>) {
    let mut reused = new_decoder(&[], dict);
    for (i, &input) in inputs.iter().enumerate() {
        *reused.get_mut() = input;
        let actual = read_all(&mut reused);
        let expected = decode_fresh(input, dict);
        assert!(
            actual == expected,
            "input {i}: reused {:?} after {} bytes, fresh {:?} after {} bytes",
            actual.1,
            actual.0.len(),
            expected.1,
            expected.0.len(),
        );
    }
}

struct Frames {
    valid: Vec<Vec<u8>>,
    malformed: Vec<(&'static str, Vec<u8>)>,
}

fn frames(dict: Option<&[u8]>) -> Frames {
    let data = multi_block_payload();
    let full = FrameInfo::new()
        .block_mode(BlockMode::Linked)
        .block_size(BlockSize::Max64KB)
        .block_checksums(true)
        .content_checksum(true)
        .content_size(Some(data.len() as u64));
    let linked = encode(full, dict, &data);
    let independent = encode(
        FrameInfo::new()
            .block_size(BlockSize::Max64KB)
            .block_checksums(true),
        dict,
        &data[BLOCK / 2..BLOCK * 2],
    );
    let small = encode(FrameInfo::new(), dict, &text(3000));

    let header_len = header(full.content_size(None), dict).len() + 8;
    let blocks = block_offsets(&linked, header_len, true);
    assert!(blocks.len() >= 4);

    let mut header_checksum = linked.clone();
    header_checksum[header_len - 1] ^= 1;

    let first_size = u32::from_le_bytes(linked[blocks[0]..blocks[0] + 4].try_into().unwrap());
    let mut block_checksum = linked.clone();
    block_checksum[blocks[0] + 4 + first_size as usize] ^= 1;

    let truncated_block = linked[..blocks[1] + 100].to_vec();
    let truncated_end = linked[..linked.len() - 8].to_vec();

    let mut content_checksum = linked.clone();
    *content_checksum.last_mut().unwrap() ^= 1;

    let mut content_size = encode(full.content_size(Some(3)), dict, b"abc");
    content_size.truncate(header_len);
    content_size.extend_from_slice(&linked[header_len..]);

    let plain = FrameInfo::new().block_size(BlockSize::Max64KB);
    let mut bad_match = header(plain, dict);
    bad_match.extend(uncompressed_block(b"hello"));
    bad_match.extend(compressed_block(&FAR_MATCH_BLOCK));
    bad_match.extend(END_MARK);

    Frames {
        valid: vec![linked, independent, small],
        malformed: vec![
            ("header checksum", header_checksum),
            ("first block checksum", block_checksum),
            ("truncated second block", truncated_block),
            ("far match in second block", bad_match),
            ("missing end mark", truncated_end),
            ("content checksum", content_checksum),
            ("content size", content_size),
        ],
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn reused_frame_decoder_matches_fresh_after_valid_frames() {
    let dict = dict();
    for dict in [None, Some(&dict[..])] {
        let frames = frames(dict);
        let [a, b, c] = [0, 1, 2].map(|i| frames.valid[i].as_slice());
        assert_reuse_matches_fresh(&[a, b, c, a, a, c, b], dict);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn reused_frame_decoder_matches_fresh_after_malformed_frames() {
    let dict = dict();
    for dict in [None, Some(&dict[..])] {
        let frames = frames(dict);
        for valid in &frames.valid {
            assert!(decode_fresh(valid, dict).1.is_ok());
        }
        for (name, malformed) in &frames.malformed {
            assert!(decode_fresh(malformed, dict).1.is_err(), "{name}");
            for valid in &frames.valid {
                let (malformed, valid) = (malformed.as_slice(), valid.as_slice());
                assert_reuse_matches_fresh(&[malformed, valid, malformed, valid], dict);
            }
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn concatenated_frames_decode_like_separate_frames() {
    let dict = dict();
    for dict in [None, Some(&dict[..])] {
        let frames = frames(dict);
        let orders: [&[usize]; 4] = [&[0, 0], &[1, 1], &[2, 2, 2], &[0, 2, 1, 0]];
        for order in orders {
            let mut stream = Vec::new();
            let mut expected = Vec::new();
            for &i in order {
                stream.extend_from_slice(&frames.valid[i]);
                expected.extend(decode_fresh(&frames.valid[i], dict).0);
            }
            assert!(
                decode_fresh(&stream, dict) == (expected, Ok(())),
                "{order:?}"
            );
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn frame_start_does_not_reach_previous_frame_history() {
    let dict = dict();
    for dict in [None, Some(&dict[..])] {
        let mut far_match = header(FrameInfo::new().block_mode(BlockMode::Linked), dict);
        far_match.extend(compressed_block(&FAR_MATCH_BLOCK));
        far_match.extend(END_MARK);
        let expected = decode_fresh(&far_match, dict);
        assert!(expected.1.is_err());

        // The first frame leaves 64 KB of wrapped linked history behind.
        let mut stream = frames(dict).valid[0].clone();
        let prefix = decode_fresh(&stream, dict).0;
        stream.extend_from_slice(&far_match);
        let (output, result) = decode_fresh(&stream, dict);
        assert_eq!(output.len(), prefix.len() + expected.0.len());
        assert_eq!(result, expected.1);
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn retry_after_block_error_does_not_resume_the_frame() {
    let data = multi_block_payload();
    let info = FrameInfo::new()
        .block_size(BlockSize::Max64KB)
        .block_checksums(true);
    let mut frame = encode(info, None, &data);
    let blocks = block_offsets(&frame, header(info, None).len(), true);
    let second_size = u32::from_le_bytes(frame[blocks[1]..blocks[1] + 4].try_into().unwrap());
    frame[blocks[1] + 4 + (second_size & 0x7FFF_FFFF) as usize] ^= 1;

    let mut decoder = FrameDecoder::new(&frame[..]);
    let mut output = Vec::new();
    let err = decoder.read_to_end(&mut output).unwrap_err();
    assert!(matches!(Error::from(err), Error::BlockChecksumError));
    assert!(output == data[..BLOCK]);

    // Reading on must not skip the bad block and finish the frame.
    let mut rest = Vec::new();
    assert!(decoder.read_to_end(&mut rest).is_err());
    assert!(rest.is_empty());
}

#[test]
fn failed_uncompressed_block_does_not_count_toward_output_limit() {
    let data = noise(100, 3);
    let info = FrameInfo::new().block_checksums(true);
    let frame = encode(info, None, &data);
    let block = header(info, None).len();
    assert_ne!(frame[block + 3] & 0x80, 0, "expected an uncompressed block");

    let mut bad_checksum = frame.clone();
    bad_checksum[block + 4 + data.len()] ^= 1;
    let truncated = frame[..block + 4 + data.len() / 2].to_vec();

    for malformed in [bad_checksum, truncated] {
        let mut decoder = FrameDecoder::with_options(
            &malformed[..],
            FrameDecoderOptions {
                max_output: Some(data.len()),
                ..Default::default()
            },
        );
        let mut output = Vec::new();
        assert!(decoder.read_to_end(&mut output).is_err());
        assert!(output.is_empty());

        // No bytes were produced, so the whole limit is still available.
        *decoder.get_mut() = &frame;
        decoder.read_to_end(&mut output).unwrap();
        assert_eq!(output, data);
    }
}

#[test]
fn linked_block_over_block_maximum_is_rejected_after_history_wraps() {
    // One literal, a 65535-byte match at offset 1, one literal: 65537 bytes,
    // one more than the 64 KB block maximum.
    let mut oversized = vec![0x1F, b'a', 1, 0];
    oversized.extend([0xFF; 256]);
    oversized.extend([236, 0x10, b'b']);
    let oversized = compressed_block(&oversized);

    let header = header(
        FrameInfo::new()
            .block_mode(BlockMode::Linked)
            .block_size(BlockSize::Max64KB),
        None,
    );
    let mut first = header.clone();
    first.extend(&oversized);
    first.extend(END_MARK);

    // Three full blocks fill the history buffer, so the fourth block
    // decodes after the window wraps to the buffer start.
    let mut fourth = header;
    for seed in 0..3 {
        fourth.extend(uncompressed_block(&noise(BLOCK, seed)));
    }
    fourth.extend(&oversized);
    fourth.extend(END_MARK);

    let (_, first_result) = decode_fresh(&first, None);
    let (output, fourth_result) = decode_fresh(&fourth, None);
    assert!(first_result.is_err());
    assert_eq!(fourth_result, first_result);
    assert_eq!(output.len(), 3 * BLOCK);
}

#[test]
fn reused_block_decompressor_matches_fresh() {
    let dict = dict();
    let message = text(2000);
    let plain = lz4rip::block::compress(&message);
    let with_dict = lz4rip::block::DictCompressor::new(&dict).compress(&message);
    let mut truncated = plain.clone();
    truncated.truncate(plain.len() / 2);
    let inputs: [&[u8]; 6] = [
        &FAR_MATCH_BLOCK,
        &plain,
        &truncated,
        &with_dict,
        &[0x10, b'a', 1, 0, 0xF0],
        &plain,
    ];

    let decoders = [
        (Decompressor::new(), None),
        (Decompressor::with_dict(&dict), Some(&dict[..])),
    ];
    for (reused, dict) in &decoders {
        let mut output = vec![0xA5; message.len()];
        for &input in &inputs {
            let actual = reused
                .decompress_into(input, &mut output)
                .map(|len| output[..len].to_vec());
            let fresh = match dict {
                Some(dict) => DecompressorRef::with_dict(dict).decompress(input, message.len()),
                None => lz4rip::block::decompress(input, message.len()),
            };
            assert_eq!(actual, fresh);
        }
    }
    assert_eq!(
        Decompressor::new().decompress(&plain, message.len()),
        Ok(message)
    );
    assert_eq!(
        Decompressor::new().decompress(&FAR_MATCH_BLOCK, 8),
        Err(DecompressError::OffsetOutOfBounds)
    );
}
