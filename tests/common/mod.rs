#![allow(dead_code)]

#[cfg(feature = "frame")]
use lz4rip::frame::BlockMode;
use lz4rip::{
    block::{decompress, decompress_append, decompress_into_uninit},
    compress as compress_block,
};

fn text_payload(target_bytes: usize) -> Vec<u8> {
    const SENTENCE: &[u8] =
        b"the quick brown fox jumps over the lazy dog; lz4 block test payload\n";
    let mut out = Vec::with_capacity(target_bytes);
    while out.len() < target_bytes {
        out.extend_from_slice(SENTENCE);
    }
    out.truncate(target_bytes);
    out
}

fn json_payload(target_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bytes);
    let mut i = 0u64;
    while out.len() < target_bytes {
        let line = format!(
            r#"{{"ts":1700000000,"level":"INFO","service":"ingest","event":{},"message":"repeatable structured payload for lz4 dictionary tests"}}"#,
            i
        );
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
        i += 1;
    }
    out.truncate(target_bytes);
    out
}

pub static COMPRESSION1K: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(725));
pub static COMPRESSION34K: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(34_308));
pub static COMPRESSION65: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(64_723));
pub static COMPRESSION66JSON: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| json_payload(66_675));

pub fn compression1k() -> &'static [u8] {
    &COMPRESSION1K
}

pub fn compression34k() -> &'static [u8] {
    &COMPRESSION34K
}

pub fn compression65() -> &'static [u8] {
    &COMPRESSION65
}

pub fn compression66json() -> &'static [u8] {
    &COMPRESSION66JSON
}

pub static DICKENS: std::sync::LazyLock<Vec<u8>> =
    std::sync::LazyLock::new(|| text_payload(10_192_446));

#[cfg(feature = "frame")]
pub fn lz4rip_frame_compress_with(
    frame_info: lz4rip::frame::FrameInfo,
    input: &[u8],
) -> Result<Vec<u8>, std::io::Error> {
    let buffer = Vec::new();
    let mut enc = lz4rip::frame::FrameEncoder::with_frame_info(frame_info, buffer);
    std::io::Write::write_all(&mut enc, input)?;
    Ok(enc.finish()?)
}

#[cfg(feature = "frame")]
pub fn lz4rip_frame_decompress(input: &[u8]) -> Result<Vec<u8>, lz4rip::frame::Error> {
    let mut de = lz4rip::frame::FrameDecoder::new(input);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut de, &mut out)?;
    Ok(out)
}

pub fn test_roundtrip(bytes: impl AsRef<[u8]>) {
    let bytes = bytes.as_ref();
    let compressed = compress_block(bytes);
    let decompressed = decompress(&compressed, bytes.len()).unwrap();
    assert_eq!(decompressed, bytes);

    let mut uninit = Vec::with_capacity(bytes.len());
    let decompressed = decompress_into_uninit(&compressed, uninit.spare_capacity_mut()).unwrap();
    assert_eq!(decompressed, bytes);

    let mut appended = b"prefix".to_vec();
    decompress_append(&compressed, &mut appended, bytes.len()).unwrap();
    assert_eq!(&appended[6..], bytes);

    #[cfg(feature = "frame")]
    for bm in &[BlockMode::Independent, BlockMode::Linked] {
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.block_mode = *bm;
        let compressed = lz4rip_frame_compress_with(frame_info, bytes).unwrap();
        let decompressed = lz4rip_frame_decompress(&compressed).unwrap();
        assert_eq!(decompressed, bytes);
    }
}
