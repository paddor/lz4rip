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
fn flush_reaches_wrapped_writer() {
    let mut enc = lz4rip::frame::FrameEncoder::new(std::io::BufWriter::new(Vec::new()));
    enc.write_all(b"hello").unwrap();
    enc.flush().unwrap();
    assert!(!enc.get_ref().get_ref().is_empty());
    assert!(enc.get_ref().buffer().is_empty());
}

#[test]
fn flush_propagates_wrapped_writer_error() {
    struct FlushError;
    impl Write for FlushError {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::WouldBlock.into())
        }
    }
    let mut enc = lz4rip::frame::FrameEncoder::new(FlushError);
    enc.write_all(b"hello").unwrap();
    assert_eq!(
        enc.flush().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn concatenated_frames_with_smaller_buffer_requirements() {
    use lz4rip::frame::{BlockMode, FrameDecoder, FrameEncoder, FrameInfo};

    for first_mode in [BlockMode::Independent, BlockMode::Linked] {
        for second_mode in [BlockMode::Independent, BlockMode::Linked] {
            let data = b"abcdefgh".repeat(40_000);
            let mut compressed = Vec::new();
            for (size, mode) in [
                (BlockSize::Max256KB, first_mode),
                (BlockSize::Max64KB, second_mode),
            ] {
                let info = FrameInfo::new().block_size(size).block_mode(mode);
                let mut enc = FrameEncoder::with_frame_info(info, Vec::new());
                enc.write_all(&data).unwrap();
                compressed.extend_from_slice(&enc.finish().unwrap());
            }
            let mut output = Vec::new();
            FrameDecoder::new(compressed.as_slice())
                .read_to_end(&mut output)
                .unwrap();
            assert_eq!(output, data.repeat(2));
        }
    }
}

#[test]
fn empty_data_blocks_are_not_eof() {
    for empty in [vec![0, 0, 0, 0x80], vec![1, 0, 0, 0, 0]] {
        let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
        enc.write_all(b"hello").unwrap();
        let mut frame = enc.finish().unwrap();
        frame.splice(7..7, empty.iter().copied().cycle().take(empty.len() * 3));

        let mut dec = lz4rip::frame::FrameDecoder::new(frame.as_slice());
        let mut output = Vec::new();
        dec.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"hello");

        let mut dec = lz4rip::frame::FrameDecoder::new(frame.as_slice());
        let mut output = [0; 16];
        let len = dec.read(&mut output).unwrap();
        assert_eq!(&output[..len], b"hello");

        // Empty blocks must not hide a missing frame terminator.
        frame.truncate(7 + empty.len() * 3);
        let mut output = Vec::new();
        assert_eq!(
            lz4rip::frame::FrameDecoder::new(frame.as_slice())
                .read_to_end(&mut output)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn encoder_errors_remain_errors_at_every_output_position() {
    struct FailOnceAfter {
        remaining: usize,
        failed: bool,
        bytes: Vec<u8>,
    }
    impl Write for FailOnceAfter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 && !self.failed {
                self.failed = true;
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            let len = if self.failed {
                buf.len()
            } else {
                buf.len().min(self.remaining).min(3)
            };
            self.bytes.extend_from_slice(&buf[..len]);
            self.remaining = self.remaining.saturating_sub(len);
            Ok(len)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let info = lz4rip::frame::FrameInfo::new()
        .block_size(BlockSize::Max64KB)
        .block_checksums(true)
        .content_checksum(true);
    let data = b"hello world ".repeat(20);
    let mut control = lz4rip::frame::FrameEncoder::with_frame_info(info, Vec::new());
    control.write_all(&data).unwrap();
    let expected = control.finish().unwrap();
    for offset in 0..=expected.len() {
        let writer = FailOnceAfter {
            remaining: offset,
            failed: false,
            bytes: Vec::new(),
        };
        let mut enc = lz4rip::frame::FrameEncoder::with_frame_info(info, writer);
        if offset == expected.len() {
            enc.write_all(&data).unwrap();
            assert_eq!(enc.finish().unwrap().bytes, expected);
            continue;
        }
        assert!(enc.write_all(&data).is_err() || enc.try_finish().is_err());
        assert_eq!(enc.get_ref().bytes, expected[..offset]);
        assert!(enc.write_all(b"retry").is_err(), "offset {offset}");
        assert!(enc.flush().is_err(), "offset {offset}");
        assert!(enc.try_finish().is_err(), "offset {offset}");
        assert_eq!(enc.get_ref().bytes, expected[..offset]);
        assert!(enc.finish().is_err(), "offset {offset}");
    }

    // Exercise a block write that happens during write_all, before finishing.
    let writer = FailOnceAfter {
        remaining: 7,
        failed: false,
        bytes: Vec::new(),
    };
    let mut enc = lz4rip::frame::FrameEncoder::with_frame_info(info, writer);
    assert!(enc.write_all(&vec![b'a'; 65_537]).is_err());
    assert!(enc.write_all(b"retry").is_err());
    assert!(enc.finish().is_err());
}

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
    let mut expected = Vec::new();
    expected.extend_from_slice(compression1k());
    expected.extend_from_slice(compression34k());
    assert_eq!(uncompressed, expected);
}

#[test]
fn concatenated_empty_frame_before_data() {
    let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
    enc.try_finish().unwrap();
    enc.write_all(compression1k()).unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::new(&*compressed);
    let mut uncompressed = Vec::new();
    dec.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(&*uncompressed, compression1k());
}

#[test]
fn concatenated_respects_total_limit() {
    let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
    enc.write_all(compression1k()).unwrap();
    enc.try_finish().unwrap();
    enc.write_all(compression34k()).unwrap();
    let compressed = enc.finish().unwrap();
    let total = compression1k().len() + compression34k().len();

    let mut dec = lz4rip::frame::FrameDecoder::with_options(
        &*compressed,
        lz4rip::frame::FrameDecoderOptions {
            max_output: Some(total),
            ..Default::default()
        },
    );
    let mut uncompressed = Vec::new();
    dec.read_to_end(&mut uncompressed).unwrap();
    assert_eq!(uncompressed.len(), total);

    let mut dec = lz4rip::frame::FrameDecoder::with_options(
        &*compressed,
        lz4rip::frame::FrameDecoderOptions {
            max_output: Some(total - 1),
            ..Default::default()
        },
    );
    let mut uncompressed = Vec::new();
    let err = dec.read_to_end(&mut uncompressed).unwrap_err();
    let inner = err
        .into_inner()
        .and_then(|e| e.downcast::<lz4rip::frame::Error>().ok());
    assert!(matches!(
        inner.as_deref(),
        Some(lz4rip::frame::Error::DecompressedSizeLimit { limit, actual })
            if *limit == total - 1 && *actual == total
    ));
    assert_eq!(uncompressed.len(), compression1k().len());
}

/// Frames of `records`, one per record, each with block and content
/// checksums in `mode`.
fn frame_per_record(records: &[&[u8]], mode: lz4rip::frame::BlockMode) -> Vec<Vec<u8>> {
    let info = lz4rip::frame::FrameInfo::new()
        .block_size(BlockSize::Max64KB)
        .block_mode(mode)
        .block_checksums(true)
        .content_checksum(true);
    records
        .iter()
        .map(|record| lz4rip_frame_compress_with(info, record).unwrap())
        .collect()
}

#[test]
fn stop_at_frame_end_reads_one_frame_per_read_to_end() {
    use lz4rip::frame::{BlockMode, FrameDecoder};

    // 66 KB spans two blocks; the empty frame reads as empty.
    let records: [&[u8]; 4] = [compression1k(), b"", compression66json(), compression34k()];
    for mode in [BlockMode::Independent, BlockMode::Linked] {
        let frames = frame_per_record(&records, mode);
        let stream = frames.concat();

        let mut dec = FrameDecoder::new(&stream[..]).stop_at_frame_end();
        for record in records {
            let mut output = Vec::new();
            assert_eq!(dec.read_to_end(&mut output).unwrap(), record.len());
            assert_eq!(output, record, "{mode:?}");
        }
        let mut output = Vec::new();
        assert_eq!(dec.read_to_end(&mut output).unwrap(), 0);
        assert_eq!(dec.read(&mut [0; 16]).unwrap(), 0);

        // The default reads every frame in one stream.
        let mut output = Vec::new();
        FrameDecoder::new(&stream[..])
            .read_to_end(&mut output)
            .unwrap();
        assert_eq!(output, records.concat(), "{mode:?}");
    }
}

#[test]
fn stop_at_frame_end_leaves_the_reader_at_the_next_frame() {
    use lz4rip::frame::{BlockMode, FrameDecoder};
    use std::io::BufRead;

    let records: [&[u8]; 2] = [compression66json(), compression1k()];
    let frames = frame_per_record(&records, BlockMode::Independent);
    let stream = frames.concat();

    let mut dec = FrameDecoder::new(&stream[..]).stop_at_frame_end();
    let mut end = 0;
    for (&record, frame) in records.iter().zip(&frames) {
        let mut output = Vec::new();
        loop {
            let chunk = dec.fill_buf().unwrap();
            if chunk.is_empty() {
                break;
            }
            // Small reads, so frame ends fall between reads too.
            let len = chunk.len().min(1000);
            output.extend_from_slice(&chunk[..len]);
            dec.consume(len);
        }
        assert_eq!(output, record);
        // The frame's end mark and content checksum are read, nothing after.
        end += frame.len();
        assert_eq!(stream.len() - dec.get_ref().len(), end);
    }
    assert!(dec.fill_buf().unwrap().is_empty());
}

#[test]
fn stop_at_frame_end_is_not_fooled_by_empty_blocks() {
    // Empty uncompressed and compressed data blocks end no frame.
    for empty in [vec![0, 0, 0, 0x80], vec![1, 0, 0, 0, 0]] {
        let mut enc = lz4rip::frame::FrameEncoder::new(Vec::new());
        enc.write_all(b"hello").unwrap();
        let mut frame = enc.finish().unwrap();
        frame.splice(7..7, empty.iter().copied().cycle().take(empty.len() * 3));
        let stream = [frame.as_slice(), &frame].concat();

        let mut dec = lz4rip::frame::FrameDecoder::new(&stream[..]).stop_at_frame_end();
        for _ in 0..2 {
            let mut output = Vec::new();
            dec.read_to_end(&mut output).unwrap();
            assert_eq!(output, b"hello");
        }
    }
}

#[test]
fn stop_at_frame_end_counts_output_across_frames() {
    let records: [&[u8]; 2] = [compression1k(), compression34k()];
    let stream = frame_per_record(&records, lz4rip::frame::BlockMode::Independent).concat();
    let total = records[0].len() + records[1].len();

    let mut dec = lz4rip::frame::FrameDecoder::with_options(
        &stream[..],
        lz4rip::frame::FrameDecoderOptions {
            max_output: Some(total - 1),
            ..Default::default()
        },
    )
    .stop_at_frame_end();
    let mut output = Vec::new();
    dec.read_to_end(&mut output).unwrap();
    assert_eq!(output, records[0]);
    let err = dec.read_to_end(&mut Vec::new()).unwrap_err();
    let inner = err
        .into_inner()
        .and_then(|e| e.downcast::<lz4rip::frame::Error>().ok());
    assert!(matches!(
        inner.as_deref(),
        Some(lz4rip::frame::Error::DecompressedSizeLimit { limit, actual })
            if *limit == total - 1 && *actual == total
    ));
}

#[test]
fn stop_at_frame_end_keeps_errors_inside_a_frame() {
    let records: [&[u8]; 2] = [compression1k(), compression34k()];
    let frames = frame_per_record(&records, lz4rip::frame::BlockMode::Independent);

    // A truncated second frame: the first reads, the second fails.
    let mut stream = frames.concat();
    stream.truncate(stream.len() - 6);
    let mut dec = lz4rip::frame::FrameDecoder::new(&stream[..]).stop_at_frame_end();
    let mut output = Vec::new();
    dec.read_to_end(&mut output).unwrap();
    assert_eq!(output, records[0]);
    let err = dec.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);

    // A wrong content checksum fails at the frame end, not after it.
    let mut stream = frames.concat();
    stream[frames[0].len() - 1] ^= 0xFF;
    let mut dec = lz4rip::frame::FrameDecoder::new(&stream[..]).stop_at_frame_end();
    let err = dec.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    // The next frame still decodes.
    let mut output = Vec::new();
    dec.read_to_end(&mut output).unwrap();
    assert_eq!(output, records[1]);
}

#[test]
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
fn dict_respects_limit() {
    let dict = b"JSON schema v1 field name= value= type= len= ".repeat(4);
    let dict_id: u32 = 0xDEADBEEF;
    let msg = b"JSON schema v1 field name=hello value=world type=str len=5";

    let mut enc =
        lz4rip::frame::FrameEncoder::with_dictionary(Vec::new(), &dict, dict_id, None).unwrap();
    enc.write_all(msg).unwrap();
    let compressed = enc.finish().unwrap();

    let mut dec = lz4rip::frame::FrameDecoder::with_options(
        &*compressed,
        lz4rip::frame::FrameDecoderOptions {
            dictionary: Some((&dict, dict_id)),
            max_output: Some(msg.len()),
        },
    );
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, msg);

    let mut dec = lz4rip::frame::FrameDecoder::with_options(
        &*compressed,
        lz4rip::frame::FrameDecoderOptions {
            dictionary: Some((&dict, dict_id)),
            max_output: Some(msg.len() - 1),
        },
    );
    let mut out = Vec::new();
    let err = dec.read_to_end(&mut out).unwrap_err();
    let inner = err
        .into_inner()
        .and_then(|e| e.downcast::<lz4rip::frame::Error>().ok());
    assert!(matches!(
        inner.as_deref(),
        Some(lz4rip::frame::Error::DecompressedSizeLimit { limit, actual })
            if *limit == msg.len() - 1 && *actual == msg.len()
    ));
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

/// A frame's first independent block under 64 KiB holds what the block API
/// writes for the same input, in both profiles.
#[test]
fn first_small_block_matches_the_block_api() {
    let mut text = Vec::new();
    for i in 0..4000 {
        text.extend_from_slice(format!("id={i} name=item-{} ok\n", i % 97).as_bytes());
    }
    for len in [100, 1000, 4096, 4097, 20_000, 65_534] {
        let input = &text[..len];
        for ratio in [false, true] {
            let enc = lz4rip::frame::FrameEncoder::new(Vec::new());
            let mut enc = if ratio { enc.ratio_profile() } else { enc };
            enc.write_all(input).unwrap();
            let frame = enc.finish().unwrap();
            let block = if ratio {
                lz4rip::block::compress_ratio(input)
            } else {
                lz4rip::block::compress(input)
            };
            // Magic, FLG, BD and header checksum, then the block size.
            let size = u32::from_le_bytes(frame[7..11].try_into().unwrap());
            assert_eq!(size as usize, block.len(), "len {len}, ratio {ratio}");
            assert_eq!(&frame[11..11 + block.len()], &block[..]);
        }
    }
}
