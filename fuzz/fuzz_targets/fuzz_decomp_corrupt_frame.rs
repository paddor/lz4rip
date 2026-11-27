#![no_main]
use libfuzzer_sys::fuzz_target;

pub fn lz4rip_frame_decompress(input: &[u8]) -> Result<Vec<u8>, lz4rip::frame::Error> {
    let mut de = lz4rip::frame::FrameDecoder::new(input);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut de, &mut out)?;
    Ok(out)
}

/// Decodes frame by frame, as readers of one frame per record do, and checks
/// that a stream that decodes whole decodes to the same bytes.
pub fn lz4rip_frame_decompress_checked(input: &[u8]) {
    let whole = lz4rip_frame_decompress(input);
    let mut de = lz4rip::frame::FrameDecoder::new(input).stop_at_frame_end();
    let mut out = Vec::new();
    // Every read takes a frame header or more, so this ends.
    let per_frame = loop {
        if de.get_ref().is_empty() {
            break Ok(out);
        }
        if let Err(e) = std::io::Read::read_to_end(&mut de, &mut out) {
            break Err(e);
        }
    };
    if let Ok(whole) = whole {
        assert_eq!(per_frame.unwrap(), whole);
    }
}

fuzz_target!(|data: &[u8]| {
    // should not panic
    lz4rip_frame_decompress_checked(data);
    let mut buffer = Vec::with_capacity(24 + data.len() * 2);
    for prefix in &[
        &[][..],                         // no prefix
        &[0x04u8, 0x22, 0x4d, 0x18][..], // magic number
    ] {
        buffer.clear();
        buffer.extend_from_slice(prefix);
        buffer.extend_from_slice(data);
        lz4rip_frame_decompress_checked(&buffer);
    }
    // magic number and correct frame header
    for prefix in &[
        &[0x04u8, 0x22, 0x4d, 0x18, 0x60, 0x40, 0x82][..], // independent
        &[0x04u8, 0x22, 0x4d, 0x18, 0x40, 0x40, 0xC0][..], // linked
    ] {
        buffer.clear();
        buffer.extend_from_slice(prefix);
        buffer.extend_from_slice(data);
        lz4rip_frame_decompress_checked(&buffer);
        // use prefix then 2 valid blocks of data
        buffer.clear();
        buffer.extend_from_slice(prefix);
        buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buffer.extend_from_slice(data);
        buffer.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buffer.extend_from_slice(data);
        lz4rip_frame_decompress_checked(&buffer);
    }
});
