#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::{Read, Write};

const MAX_DATA_SIZE: usize = 64 * 1024;

#[derive(Clone, Debug, arbitrary::Arbitrary)]
pub struct Input {
    sample: Vec<u8>,
    data_size_seed: usize,
    chunk_size_seed: usize,
}

fuzz_target!(|input: Input| {
    let Input {
        sample,
        data_size_seed,
        chunk_size_seed,
    } = input;
    if sample.is_empty() {
        return;
    }
    let chunk_size = (chunk_size_seed % MAX_DATA_SIZE).max(64);
    let data_size = data_size_seed % MAX_DATA_SIZE;

    let mut data = Vec::with_capacity(data_size);
    while data.len() < data_size {
        data.extend_from_slice(&sample);
    }
    data.truncate(data_size);

    for bm in &[
        lz4rip::frame::BlockMode::Independent,
        lz4rip::frame::BlockMode::Linked,
    ] {
        for bs in &[
            lz4rip::frame::BlockSize::Max64KB,
            lz4rip::frame::BlockSize::Max256KB,
            lz4rip::frame::BlockSize::Max1MB,
            lz4rip::frame::BlockSize::Max4MB,
        ] {
            for (check_sum, ratio) in [(true, false), (false, false), (true, true)] {
                // io::Write
                let mut fi = lz4rip::frame::FrameInfo::default();
                fi.block_mode = *bm;
                fi.block_size = *bs;
                fi.block_checksums = check_sum;
                fi.content_checksum = check_sum;
                let mut enc =
                    lz4rip::frame::FrameEncoder::with_frame_info(fi, Vec::with_capacity(data_size));
                if ratio {
                    enc = enc.ratio_profile();
                }
                for chunk in data.chunks(chunk_size) {
                    enc.write(chunk).unwrap();
                    // by flushing we force encoder to output a frame block
                    // if buffered data <= max block size
                    enc.flush().unwrap();
                }
                let compressed = enc.finish().unwrap();
                // io::Read
                let mut decompressed = Vec::new();
                decompressed.resize(data.len() + chunk_size, 0);
                let mut pos = 0;
                let mut dec = lz4rip::frame::FrameDecoder::new(&*compressed);
                loop {
                    match dec.read(&mut decompressed[pos..pos + chunk_size]).unwrap() {
                        0 => {
                            decompressed.truncate(pos);
                            break;
                        }
                        i => {
                            pos += i;
                        }
                    }
                }
                assert_eq!(data, decompressed);
            }
        }
    }
});
