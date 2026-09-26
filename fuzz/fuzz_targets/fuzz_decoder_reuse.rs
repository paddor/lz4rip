#![no_main]
//! A reused decoder must decode every input exactly like a fresh decoder,
//! even after decoding other, possibly malformed, inputs.
//!
//! `FrameDecoder` is reused by replacing its reader through `get_mut`. Block
//! `Decompressor`s are reused together with their dirty output buffer.
use std::io::Read;
use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;
use lz4rip::block::{DecompressError, Decompressor};
use lz4rip::frame::{BlockMode, BlockSize, FrameDecoder, FrameEncoder, FrameInfo};

/// Output bytes read per frame input. A read that reaches this limit leaves
/// its frame unfinished, so the reused decoder is replaced afterwards.
const LIMIT: usize = 1 << 20;
const BLOCK_CAPACITY: usize = 1 << 16;
const DICT: &[u8] = b"level=INFO service=ingest event= message=lz4rip decoder reuse";
const DICT_ID: u32 = 0x0D1C_7001;

/// Frame headers to prepend to the fuzz input. The first one is empty.
static HEADERS: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    let mut headers = vec![Vec::new()];
    for mode in [BlockMode::Independent, BlockMode::Linked] {
        for checksums in [false, true] {
            for dict in [None, Some(DICT)] {
                let info = FrameInfo::new()
                    .block_mode(mode)
                    .block_size(BlockSize::Max64KB)
                    .block_checksums(checksums)
                    .content_checksum(checksums);
                let encoder = match dict {
                    Some(dict) => {
                        FrameEncoder::with_dictionary(Vec::new(), dict, DICT_ID, Some(info))
                            .unwrap()
                    }
                    None => FrameEncoder::with_frame_info(info, Vec::new()),
                };
                // An empty frame is its header, the end mark, and an
                // optional content checksum.
                let frame = encoder.finish().unwrap();
                let trailer = if checksums { 8 } else { 4 };
                headers.push(frame[..frame.len() - trailer].to_vec());
            }
        }
    }
    headers
});

type Outcome = (Vec<u8>, Result<(), String>);

fn new_frame_decoder(input: &[u8], dict: bool) -> FrameDecoder<&[u8]> {
    if dict {
        FrameDecoder::with_dictionary(input, DICT, DICT_ID)
    } else {
        FrameDecoder::new(input)
    }
}

fn read_frames(decoder: &mut FrameDecoder<&[u8]>) -> Outcome {
    let mut output = Vec::new();
    let result = decoder
        .by_ref()
        .take(LIMIT as u64)
        .read_to_end(&mut output)
        .map(drop)
        .map_err(|e| e.to_string());
    (output, result)
}

fn decode_block(
    decompressor: &Decompressor,
    input: &[u8],
    output: &mut [u8],
) -> Result<Vec<u8>, DecompressError> {
    decompressor
        .decompress_into(input, output)
        .map(|len| output[..len].to_vec())
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let Some((&split, rest)) = rest.split_first() else {
        return;
    };
    let (first, second) = rest.split_at(usize::from(split) * rest.len() / 255);

    let headers = &*HEADERS;
    let frames = [
        [&headers[usize::from(selector & 0xF) % headers.len()], first].concat(),
        [&headers[usize::from(selector >> 4) % headers.len()], second].concat(),
    ];
    for dict in [false, true] {
        let mut reused = new_frame_decoder(&[], dict);
        for input in [&frames[0], &frames[1], &frames[0], &frames[1]] {
            let expected = read_frames(&mut new_frame_decoder(input, dict));
            *reused.get_mut() = input;
            let actual = read_frames(&mut reused);
            assert!(
                actual == expected,
                "reused {:?} after {} bytes, fresh {:?} after {} bytes",
                actual.1,
                actual.0.len(),
                expected.1,
                expected.0.len(),
            );
            if actual.0.len() == LIMIT {
                reused = new_frame_decoder(&[], dict);
            }
        }
    }

    let new_decompressors: [fn() -> Decompressor; 2] =
        [Decompressor::new, || Decompressor::with_dict(DICT)];
    for new_decompressor in new_decompressors {
        let reused = new_decompressor();
        let mut output = vec![0xA5; BLOCK_CAPACITY];
        for input in [first, second, first, second] {
            let expected = decode_block(&new_decompressor(), input, &mut [0; BLOCK_CAPACITY]);
            assert_eq!(decode_block(&reused, input, &mut output), expected);
        }
    }
});
