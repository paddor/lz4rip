#![no_main]
//! `FrameDecoder::read_to_end` decodes blocks straight into its buffer;
//! `read` decodes them into the decoder's own. Both must return the same
//! bytes and fail on the same inputs, for arbitrary bytes and for valid
//! frames of any settings, also when `read` consumed part of a frame first.
use libfuzzer_sys::fuzz_target;
use lz4rip::frame::{BlockMode, BlockSize, FrameDecoder, FrameEncoder, FrameInfo};
use std::io::{Read, Write};

/// Decodes with `read` calls of `chunk` bytes.
fn read_loop(input: &[u8], chunk: usize) -> (Vec<u8>, bool) {
    let mut dec = FrameDecoder::new(input);
    let mut out = Vec::new();
    let mut buf = vec![0; chunk];
    loop {
        match dec.read(&mut buf) {
            Ok(0) => return (out, true),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(_) => return (out, false),
        }
    }
}

/// Decodes with one `read` of `first` bytes, then `read_to_end` after a
/// prefix that blocks must not refer to.
fn read_then_to_end(input: &[u8], first: usize) -> (Vec<u8>, bool) {
    let mut dec = FrameDecoder::new(input);
    let mut out = b"prefix".to_vec();
    let mut head = vec![0; first];
    let ok = match dec.read(&mut head) {
        Ok(n) => {
            out.extend_from_slice(&head[..n]);
            dec.read_to_end(&mut out).is_ok()
        }
        Err(_) => false,
    };
    (out.split_off(6), ok)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let (ctl, rest) = data.split_at(4);
    let chunk = ctl[0] as usize % 200 + 1;
    let first = ctl[1] as usize * 300;
    let info = FrameInfo::new()
        .block_mode(if ctl[2] & 1 != 0 {
            BlockMode::Linked
        } else {
            BlockMode::Independent
        })
        .block_size(BlockSize::Max64KB)
        .block_checksums(ctl[2] & 2 != 0)
        .content_checksum(ctl[2] & 4 != 0);
    // Repeated, so frames span several blocks.
    let content = rest.repeat(ctl[3] as usize % 48 + 1);
    let info = if ctl[2] & 8 != 0 {
        info.content_size(Some(content.len() as u64))
    } else {
        info
    };
    let mut enc = FrameEncoder::with_frame_info(info, Vec::new());
    enc.write_all(&content).unwrap();
    let frame = enc.finish().unwrap();
    // A valid frame, two of them, a cut one, and the raw bytes.
    let two = [&frame[..], &frame[..]].concat();
    let cut = &frame[..frame.len() - 1 - ctl[0] as usize % frame.len().min(64)];
    for input in [&frame[..], &two[..], cut, rest] {
        let reference = read_loop(input, chunk);
        let mut out = b"abc".to_vec();
        let ok = FrameDecoder::new(input).read_to_end(&mut out).is_ok();
        assert_eq!(&out[..3], b"abc");
        assert!(out[3..] == reference.0[..] && ok == reference.1);
        assert!(read_then_to_end(input, first) == reference);
    }
    assert!(read_loop(&frame, chunk) == (content, true));
});
