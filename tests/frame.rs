#![cfg(feature = "frame")]

mod common;

use common::*;
use lz4rip::frame::BlockSize;
use std::io::{Read, Write};

const LINKED_DICT_ID: u32 = 1;
const LINKED_DICT_ROUNDTRIP_REPEAT_COUNT: usize = 3000;
const LINKED_DICT_FRAME_BLOCK_BYTES: usize = 64 * 1024;
const LINKED_DICT_REPEAT_START: usize = LINKED_DICT_FRAME_BLOCK_BYTES / 2;

#[test]
fn concatenated() {
    let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
    enc.write_all(compression1k()).unwrap();
    enc.try_finish().unwrap();
    enc.write_all(compression34k()).unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::new(&*compressed);
    let mut uncompressed = Vec::new();
    dec.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(&*uncompressed, compression1k());
    uncompressed.clear();
    dec.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(&*uncompressed, compression34k());
}

#[test]
#[cfg_attr(miri, ignore)]
fn checksums() {
    for &input in &[compression34k(), compression66json()] {
        // Block checksum
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.block_checksums = true;
        let mut compressed = lz4rip_frame_compress_with(frame_info, input).unwrap();
        let uncompressed = lz4rip_frame_decompress(&compressed).unwrap();
        assert_eq!(uncompressed, input);
        // Corrupt last block checksum (8th to 4th last bytes).
        let compressed_len = compressed.len();
        compressed[compressed_len - 5] ^= 0xFF;
        match lz4rip_frame_decompress(&compressed) {
            Err(lz4rip::frame::Error::BlockChecksumError) => (),
            r => panic!("{:?}", r),
        }

        // Content checksum
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.content_checksum = true;
        let mut compressed = lz4rip_frame_compress_with(frame_info, input).unwrap();
        let uncompressed = lz4rip_frame_decompress(&compressed).unwrap();
        assert_eq!(uncompressed, input);
        // Corrupt content checksum (last 4 bytes).
        let compressed_len = compressed.len();
        compressed[compressed_len - 1] ^= 0xFF;
        match lz4rip_frame_decompress(&compressed) {
            Err(lz4rip::frame::Error::ContentChecksumError) => (),
            r => panic!("{:?}", r),
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn block_size() {
    let mut last_compressed_len = usize::MAX;
    for block_size in &[
        BlockSize::Max64KB,
        BlockSize::Max256KB,
        BlockSize::Max1MB,
        BlockSize::Max4MB,
    ] {
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.block_size = *block_size;
        let compressed = lz4rip_frame_compress_with(frame_info, &DICKENS).unwrap();

        let uncompressed = lz4rip_frame_decompress(&compressed).unwrap();
        assert_eq!(uncompressed, *DICKENS);

        // For large input (dickens, 10 MB), strictly better compression with larger block size.
        assert!(compressed.len() < last_compressed_len);
        last_compressed_len = compressed.len();
    }
}

#[test]
fn content_size() {
    let mut frame_info = lz4rip::frame::FrameInfo::new();
    frame_info.content_size = Some(compression1k().len() as u64);
    let mut compressed = lz4rip_frame_compress_with(frame_info, compression1k()).unwrap();

    let uncompressed = lz4rip_frame_decompress(&compressed).unwrap();
    assert_eq!(uncompressed, compression1k());

    // Corrupt the content size in the header.
    {
        let mut frame_info = lz4rip::frame::FrameInfo::new();
        frame_info.content_size = Some(3);
        let dummy_compressed = lz4rip_frame_compress_with(frame_info, b"123").unwrap();
        // 15 (7 + 8) is the header size plus content size field.
        compressed[..15].copy_from_slice(&dummy_compressed[..15]);
    }
    match lz4rip_frame_decompress(&compressed) {
        Err(lz4rip::frame::Error::ContentLengthError { expected, actual }) => {
            assert_eq!(expected, 3);
            assert_eq!(actual, 725);
        }
        r => panic!("{:?}", r),
    }
}

#[test]
fn dict_round_trip() {
    let dict = b"JSON schema v1 field name= value= type= len= ".repeat(4);
    let dict_id: u32 = 0xDEADBEEF;
    let msg = b"JSON schema v1 field name=hello value=world type=str len=5";

    let mut enc =
        lz4rip::frame::FrameEncoder::with_dictionary(Vec::new(), &dict, dict_id, None).unwrap();
    enc.write_all(msg).unwrap();
    let compressed = enc.finish().unwrap();

    assert_eq!(&compressed[..4], &[0x04, 0x22, 0x4d, 0x18]);

    let mut dec = lz4rip::frame::FrameDecoder::with_dictionary(&*compressed, &dict, dict_id);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, msg);
}

#[test]
fn dict_with_frame_info_round_trip_and_preserves_settings() {
    let dict = b"JSON schema v1 field name= value= type= len= ".repeat(4);
    let dict_id = 0xDEAD_BEEF;
    let msg = b"JSON schema v1 field name=hello value=world type=str len=5";
    let frame_info = lz4rip::frame::FrameInfo::new()
        .block_mode(lz4rip::frame::BlockMode::Independent)
        .block_size(lz4rip::frame::BlockSize::Max256KB)
        .block_checksums(true)
        .content_checksum(true)
        .content_size(Some(msg.len() as u64));

    let mut enc =
        lz4rip::frame::FrameEncoder::with_dictionary(Vec::new(), &dict, dict_id, Some(frame_info))
            .unwrap();
    assert_eq!(enc.frame_info().block_mode, frame_info.block_mode);
    assert_eq!(enc.frame_info().block_size, frame_info.block_size);
    assert_eq!(enc.frame_info().block_checksums, frame_info.block_checksums);
    assert_eq!(
        enc.frame_info().content_checksum,
        frame_info.content_checksum
    );
    assert_eq!(enc.frame_info().content_size, frame_info.content_size);

    enc.write_all(msg).unwrap();
    let compressed = enc.finish().unwrap();
    let mut dec = lz4rip::frame::FrameDecoder::with_dictionary(&*compressed, &dict, dict_id);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, msg);
}

#[test]
fn dict_with_linked_blocks_round_trip() {
    let frame_info = lz4rip::frame::FrameInfo::new().block_mode(lz4rip::frame::BlockMode::Linked);
    let dict = b"event_type=metric service=ingest shard= value= ".repeat(16);
    let msg = b"event_type=metric service=ingest shard=3 value=42\n"
        .repeat(LINKED_DICT_ROUNDTRIP_REPEAT_COUNT);

    let mut enc = lz4rip::frame::FrameEncoder::with_dictionary(
        Vec::new(),
        &dict,
        LINKED_DICT_ID,
        Some(frame_info),
    )
    .unwrap();
    enc.write_all(&msg).unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::with_dictionary(&*compressed, &dict, LINKED_DICT_ID);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, msg);
}

#[test]
fn linked_blocks_improve_multi_block_ratio_with_dictionary() {
    let dict = b"prefix=orders region=west status=complete payload=".repeat(32);
    let first_block = &compression66json()[..LINKED_DICT_FRAME_BLOCK_BYTES];
    let mut msg = first_block.to_vec();
    msg.extend_from_slice(&first_block[LINKED_DICT_REPEAT_START..]);
    msg.extend_from_slice(&first_block[LINKED_DICT_REPEAT_START..]);

    let independent_info = lz4rip::frame::FrameInfo::new()
        .block_mode(lz4rip::frame::BlockMode::Independent)
        .block_size(lz4rip::frame::BlockSize::Max64KB);
    let linked_info = lz4rip::frame::FrameInfo::new()
        .block_mode(lz4rip::frame::BlockMode::Linked)
        .block_size(lz4rip::frame::BlockSize::Max64KB);

    let mut independent = lz4rip::frame::FrameEncoder::with_dictionary(
        Vec::new(),
        &dict,
        LINKED_DICT_ID,
        Some(independent_info),
    )
    .unwrap();
    independent.write_all(&msg).unwrap();
    let independent = independent.finish().unwrap();

    let mut linked = lz4rip::frame::FrameEncoder::with_dictionary(
        Vec::new(),
        &dict,
        LINKED_DICT_ID,
        Some(linked_info),
    )
    .unwrap();
    linked.write_all(&msg).unwrap();
    let linked = linked.finish().unwrap();

    assert!(linked.len() < independent.len());
}

#[test]
fn dict_id_mismatch_fails() {
    let dict = b"prefix AAA ".repeat(8);
    let msg = b"prefix AAA tail";
    let mut enc =
        lz4rip::frame::FrameEncoder::with_dictionary(Vec::new(), &dict, 0xAAAA_AAAA, None).unwrap();
    enc.write_all(msg).unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::with_dictionary(&*compressed, &dict, 0xBBBB_BBBB);
    let mut out = Vec::new();
    let err = dec.read_to_end(&mut out).unwrap_err();
    let inner = err
        .into_inner()
        .and_then(|e| e.downcast::<lz4rip::frame::Error>().ok());
    match inner.as_deref() {
        Some(lz4rip::frame::Error::DictIdMismatch { .. }) => {}
        other => panic!("expected DictIdMismatch, got {other:?}"),
    }
}

#[test]
fn dict_required_when_frame_declares_one() {
    let dict = b"common ".repeat(8);
    let mut enc = lz4rip::frame::FrameEncoder::with_dictionary(Vec::new(), &dict, 1, None).unwrap();
    enc.write_all(b"common payload").unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::new(&*compressed);
    let mut out = Vec::new();
    let err = dec.read_to_end(&mut out).unwrap_err();
    let inner = err
        .into_inner()
        .and_then(|e| e.downcast::<lz4rip::frame::Error>().ok());
    assert!(matches!(
        inner.as_deref(),
        Some(lz4rip::frame::Error::DictionaryNotSupported)
    ));
}

#[test]
fn truncated_standard_frame_is_error() {
    let frame_info = lz4rip::frame::FrameInfo::new();
    let compressed = lz4rip_frame_compress_with(frame_info, compression34k()).unwrap();
    let truncated = &compressed[..compressed.len() - 4];
    let err = lz4rip_frame_decompress(truncated).unwrap_err();
    assert!(
        matches!(err, lz4rip::frame::Error::IoError(_)),
        "expected IoError(UnexpectedEof), got {err:?}"
    );
}

#[test]
fn try_finish_idempotent() {
    let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
    std::io::Write::write_all(&mut enc, b"hello").unwrap();
    enc.try_finish().unwrap();
    let first = enc.get_ref().clone();
    enc.try_finish().unwrap();
    assert_eq!(
        enc.get_ref(),
        &first,
        "double try_finish must not append bytes"
    );
}

/// BlockSize::Auto resolves to a concrete size based on the first write.
#[test]
#[cfg_attr(miri, ignore)]
fn block_size_auto_resolution() {
    use lz4rip::frame::{BlockSize, FrameDecoder, FrameEncoder, FrameInfo};

    for (write_len, expected_block_size) in [
        (100, BlockSize::Max64KB),
        (64 * 1024, BlockSize::Max64KB),
        (65 * 1024, BlockSize::Max256KB),
        (256 * 1024, BlockSize::Max256KB),
        (257 * 1024, BlockSize::Max4MB),
        (1024 * 1024, BlockSize::Max4MB),
        (1025 * 1024, BlockSize::Max4MB),
    ] {
        let input: Vec<u8> = (0u8..=255).cycle().take(write_len).collect();
        let mut info = FrameInfo::new();
        info.block_size = BlockSize::Auto;
        let mut enc = FrameEncoder::with_frame_info(info, Vec::new());
        std::io::Write::write_all(&mut enc, &input).unwrap();
        assert_eq!(
            enc.frame_info().block_size,
            expected_block_size,
            "Auto resolved to wrong block size for write_len={write_len}"
        );
        let compressed = enc.finish().unwrap();

        let mut dec = FrameDecoder::new(&compressed[..]);
        let mut decompressed = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut decompressed).unwrap();
        assert_eq!(
            decompressed, input,
            "roundtrip failed for write_len={write_len}"
        );
    }
}

#[test]
fn push_decode_roundtrip() {
    use lz4rip::frame::push::*;

    let input = compression34k();
    let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
    enc.write_all(input).unwrap();
    let compressed = enc.finish().unwrap();

    let mut pos = 0;

    let hdr_size = frame_header_size(&compressed[pos..]).unwrap();
    let info = parse_frame_header(&compressed[pos..pos + hdr_size]).unwrap();
    pos += hdr_size;

    let max_block = info.block_size.get_size();
    let mut output = Vec::new();

    loop {
        let block =
            parse_block_header(compressed[pos..pos + BLOCK_HEADER_SIZE].try_into().unwrap())
                .unwrap();
        pos += BLOCK_HEADER_SIZE;

        match block {
            BlockHeader::Compressed(len) => {
                let len = len as usize;
                let mut buf = vec![0u8; max_block];
                let n =
                    lz4rip::block::decompress_into(&compressed[pos..pos + len], &mut buf).unwrap();
                output.extend_from_slice(&buf[..n]);
                pos += len;
            }
            BlockHeader::Uncompressed(len) => {
                let len = len as usize;
                output.extend_from_slice(&compressed[pos..pos + len]);
                pos += len;
            }
            BlockHeader::EndMark => break,
        }
    }

    assert_eq!(output, input);
}

#[test]
fn push_decode_linked_blocks() {
    use lz4rip::frame::push::*;

    let input = compression66json();
    let frame_info = lz4rip::frame::FrameInfo::new().block_mode(lz4rip::frame::BlockMode::Linked);
    let mut enc = lz4rip::frame::FrameEncoder::with_frame_info(frame_info, Vec::new());
    enc.write_all(input).unwrap();
    let compressed = enc.finish().unwrap();

    let mut pos = 0;

    let hdr_size = frame_header_size(&compressed[pos..]).unwrap();
    let info = parse_frame_header(&compressed[pos..pos + hdr_size]).unwrap();
    assert_eq!(info.block_mode, lz4rip::frame::BlockMode::Linked);
    pos += hdr_size;

    let max_block = info.block_size.get_size();
    let mut output = Vec::new();

    loop {
        let block =
            parse_block_header(compressed[pos..pos + BLOCK_HEADER_SIZE].try_into().unwrap())
                .unwrap();
        pos += BLOCK_HEADER_SIZE;

        match block {
            BlockHeader::Compressed(len) => {
                let len = len as usize;
                let mut buf = vec![0u8; max_block];
                let dict_start = output.len().saturating_sub(64 * 1024);
                let n = lz4rip::block::decompress_into_with_dict(
                    &compressed[pos..pos + len],
                    &mut buf,
                    &output[dict_start..],
                )
                .unwrap();
                output.extend_from_slice(&buf[..n]);
                pos += len;
            }
            BlockHeader::Uncompressed(len) => {
                let len = len as usize;
                output.extend_from_slice(&compressed[pos..pos + len]);
                pos += len;
            }
            BlockHeader::EndMark => break,
        }
    }

    assert_eq!(output, input);
}
