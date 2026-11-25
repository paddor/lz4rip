#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Read, Write};

use lz4rip::frame::{FrameDecoder, FrameEncoder};

const MAX_DATA_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug, arbitrary::Arbitrary)]
struct Input {
    sample: Vec<u8>,
    dict: Vec<u8>,
    data_size_seed: usize,
    chunk_size_seed: usize,
}

fuzz_target!(|input: Input| {
    if input.sample.is_empty() {
        return;
    }
    let chunk_size = (input.chunk_size_seed % MAX_DATA_SIZE).max(64);
    let data_size = input.data_size_seed % MAX_DATA_SIZE;

    let mut data = Vec::with_capacity(data_size);
    while data.len() < data_size {
        data.extend_from_slice(&input.sample);
    }
    data.truncate(data_size);

    let dict_len = input.dict.len().min(64 * 1024);
    let dict = &input.dict[..dict_len];
    if dict.is_empty() {
        return;
    }

    let dict_id = 0x1234_5678;

    // Compress with dict (Independent block mode, default block size).
    let mut enc =
        FrameEncoder::with_dictionary(Vec::with_capacity(data.len()), dict, dict_id, None).unwrap();
    for chunk in data.chunks(chunk_size) {
        enc.write_all(chunk).unwrap();
    }
    let compressed = enc.finish().unwrap();

    // Decompress with correct dict: must round-trip.
    let mut dec = FrameDecoder::with_dictionary(&*compressed, dict, dict_id);
    let mut decompressed = Vec::new();
    dec.read_to_end(&mut decompressed).unwrap();
    assert_eq!(data, decompressed);

    // Ratio profile, linked blocks: must round-trip.
    let info = lz4rip::frame::FrameInfo::new().block_mode(lz4rip::frame::BlockMode::Linked);
    let mut enc = FrameEncoder::with_dictionary(Vec::new(), dict, dict_id, Some(info))
        .unwrap()
        .ratio_profile();
    for chunk in data.chunks(chunk_size) {
        enc.write_all(chunk).unwrap();
    }
    let ratio = enc.finish().unwrap();
    let mut dec = FrameDecoder::with_dictionary(&*ratio, dict, dict_id);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(data, out);

    // Decompress with wrong dict: must not panic.
    let wrong_dict: Vec<u8> = dict.iter().map(|b| b.wrapping_add(1)).collect();
    let mut dec2 = FrameDecoder::with_dictionary(&*compressed, &wrong_dict, dict_id);
    let mut out2 = Vec::new();
    let _ = dec2.read_to_end(&mut out2);

    // Decompress without dict: must not panic (should error on dict_id mismatch).
    let mut dec3 = FrameDecoder::new(&*compressed);
    let mut out3 = Vec::new();
    let _ = dec3.read_to_end(&mut out3);
});
