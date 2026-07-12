//! Fast Rust implementation of LZ4 compression.
//!
//! # Overview
//!
//! This crate provides two ways to use lz4. The first is through
//! [`frame::FrameDecoder`] and [`frame::FrameEncoder`],
//! which implement the `std::io::Read` and `std::io::Write` traits with the
//! lz4 frame format. The frame format supports streaming compression and
//! decompression.
//!
//! The second way is through the [`compress`](block/fn.compress.html) and
//! [`decompress`](block/fn.decompress.html) functions. These provide access to
//! the lz4 block format without framing overhead.
//!
//! # Example: compress data on `stdin` with frame format
//! This program reads data from `stdin`, compresses it and emits it to `stdout`.
//! ```no_run
//! # #[cfg(feature = "frame")]
//! # {
//! use std::io;
//! let stdin = io::stdin();
//! let stdout = io::stdout();
//! let mut rdr = stdin.lock();
//! // Wrap the stdout writer in a LZ4 Frame writer.
//! let mut wtr = lz4rip::frame::FrameEncoder::new(stdout.lock());
//! io::copy(&mut rdr, &mut wtr).expect("I/O operation failed");
//! wtr.finish().unwrap();
//! # }
//! ```
//! # Example: decompress data on `stdin` with frame format
//! This program reads data from `stdin`, decompresses it and emits it to `stdout`.
//! ```no_run
//! # #[cfg(feature = "frame")]
//! # {
//! use std::io;
//! let stdin = io::stdin();
//! let stdout = io::stdout();
//! // Wrap the stdin reader in a LZ4 FrameDecoder.
//! let mut rdr = lz4rip::frame::FrameDecoder::new(stdin.lock());
//! let mut wtr = stdout.lock();
//! io::copy(&mut rdr, &mut wtr).expect("I/O operation failed");
//! # }
//! ```
//!
//! # Example: block format roundtrip
//! ```
//! use lz4rip::block::{compress, decompress};
//! let input: &[u8] = b"Hello people, what's up?";
//! let compressed = compress(input);
//! let uncompressed = decompress(&compressed, input.len()).unwrap();
//! assert_eq!(input, uncompressed);
//! ```
//!
//! ## Feature Flags
//!
//! - `frame` support for LZ4 frame format. _implies `std`, enabled by default_
//! - `std` enables dependency on the standard library. _enabled by default_
//!
//! For no_std support only the [`block format`](block/index.html) is supported.
//!
//!
#![forbid(unsafe_code)]
#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]

/// LZ4 block format. Works in `no_std` and no-alloc environments.
///
/// See the [spec](https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md).
///
/// Without the `alloc` feature, only [`decompress_into`],
/// [`decompress_into_with_dict`], and [`DecompressorRef`] are available.
///
/// # Example: block format roundtrip
/// ```
/// use lz4rip::block::{compress, decompress};
/// let input: &[u8] = b"Hello people, what's up?";
/// let compressed = compress(input);
/// let uncompressed = decompress(&compressed, input.len()).unwrap();
/// assert_eq!(input, uncompressed);
/// ```
pub mod block {
    pub use lz4rip_core::{CompressError, DecompressError};
    pub use lz4rip_decode::{DecompressorRef, decompress_into, decompress_into_with_dict};
    pub use lz4rip_encode::{
        CompressorRef, CompressorRefN, DEFAULT_DICT_ENTRIES, DEFAULT_NODICT_ENTRIES,
        DictCompressorRef, DictCompressorRefN, MIN_ENTRIES, compress_into, compress_into_with_dict,
        get_maximum_output_size,
    };

    #[cfg(feature = "alloc")]
    pub use lz4rip_decode::{Decompressor, decompress};
    #[cfg(feature = "alloc")]
    pub use lz4rip_encode::{
        Compressor, CompressorN, DictCompressor, DictCompressorN, DictTrainer, compress,
    };
}

#[cfg(feature = "frame")]
#[cfg_attr(docsrs, doc(cfg(feature = "frame")))]
pub mod frame;

#[cfg(feature = "alloc")]
pub use block::{compress, decompress};
pub use block::{
    compress_into, compress_into_with_dict, decompress_into, decompress_into_with_dict,
    get_maximum_output_size,
};

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::{format, vec, vec::Vec};
    use lz4rip_core::SliceSink;

    #[test]
    fn integer_roundtrip() {
        for value in [0, 1, 254, 255, 256, 1000, 65535, 100_000] {
            let mut buf = vec![0u8; value / 255 + 2];
            let mut sink = SliceSink::new(&mut buf, 0);
            lz4rip_encode::write_integer(&mut sink, value);

            let decoded = lz4rip_decode::read_integer(&buf, &mut 0).unwrap();
            assert_eq!(value, decoded);
        }
    }

    #[test]
    fn dict_roundtrip() {
        let input: &[u8] = &[
            10, 12, 14, 16, 18, 10, 12, 14, 16, 18, 10, 12, 14, 16, 18, 10, 12, 14, 16, 18,
        ];
        let dict = input;
        let mut comp = lz4rip_encode::DictCompressor::new(dict);
        let compressed = comp.compress(input);
        assert!(compressed.len() < lz4rip_encode::compress(input).len());

        let decomp = lz4rip_decode::Decompressor::with_dict(dict);
        let uncompressed = decomp.decompress(&compressed, input.len()).unwrap();
        assert_eq!(input, &uncompressed[..]);
    }

    #[test]
    fn dict_improves_compression() {
        fn json_msg(i: u32) -> Vec<u8> {
            format!(
                r#"{{"ts":"2026-04-27T12:00:00.{i:04}Z","level":"INFO","service":"api-gw","trace":"{i:08x}","method":"GET","path":"/v1/users/{i:04}","status":200,"latency_ms":{lat},"region":"us-east-1"}}"#,
                i = i,
                lat = 10 + i % 490,
            )
            .into_bytes()
        }

        let mut trainer = lz4rip_encode::DictTrainer::new(2048);
        for i in 0..200 {
            trainer.add_sample(&json_msg(i));
        }
        let dict = trainer.train();
        assert!(!dict.is_empty());

        let mut compressor = lz4rip_encode::DictCompressor::new(&dict);
        let decompressor = lz4rip_decode::Decompressor::with_dict(&dict);

        let test_msg = json_msg(9999);
        let compressed_with = compressor.compress(&test_msg);
        let compressed_without = lz4rip_encode::compress(&test_msg);

        assert!(
            compressed_with.len() < compressed_without.len(),
            "dict compressed {} >= no-dict {}",
            compressed_with.len(),
            compressed_without.len()
        );

        let mut decomp_buf = vec![0u8; test_msg.len()];
        let n = decompressor
            .decompress_into(&compressed_with, &mut decomp_buf)
            .unwrap();
        assert_eq!(&decomp_buf[..n], &test_msg[..]);
    }

    #[test]
    fn miri_wildcopy_roundtrip() {
        let mut state: u64 = 0x1234_5678_9abc_def1;
        let mut rng = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..40 {
            let len = (rng() % 600) as usize + 64;
            let alpha = 1 + (rng() % 40);
            let data: Vec<u8> = (0..len).map(|_| (rng() % alpha) as u8).collect();
            let max = lz4rip_encode::get_maximum_output_size(data.len());
            let mut comp = vec![0u8; max];
            let comp_len = lz4rip_encode::compress_into(&data, &mut comp).unwrap();
            let decomp = lz4rip_decode::decompress(&comp[..comp_len], data.len()).unwrap();
            assert_eq!(data, decomp);
        }
    }
}
