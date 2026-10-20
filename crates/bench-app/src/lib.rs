//! Stand-in for an application's library crate.
//!
//! `lz4rip_bench` calls every Rust codec through this crate, so their generic
//! and `#[inline]` code compiles in a library crate, as in applications.
//! Compiled in the benchmark binary instead, the compiler sees every call and
//! specializes the codecs on the benchmark's own arguments, which applications
//! do not get.

#![forbid(unsafe_code)]

/// lz4rip block API.
pub mod lz4rip {
    use lz4rip::block::{CompressError, DecompressError};

    /// [`lz4rip::block::compress_into`].
    pub fn compress_into(input: &[u8], output: &mut [u8]) -> Result<usize, CompressError> {
        lz4rip::block::compress_into(input, output)
    }

    /// [`lz4rip::block::decompress_into`].
    pub fn decompress_into(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
        lz4rip::block::decompress_into(input, output)
    }

    /// [`lz4rip::block::Compressor`].
    #[derive(Debug, Default)]
    pub struct Compressor(lz4rip::block::Compressor);

    impl Compressor {
        /// Create a compressor.
        pub fn new() -> Self {
            Self(lz4rip::block::Compressor::new())
        }

        /// Compress `input` into `output`.
        pub fn compress_into(
            &mut self,
            input: &[u8],
            output: &mut [u8],
        ) -> Result<usize, CompressError> {
            self.0.compress_into(input, output)
        }
    }

    /// [`lz4rip::block::DictCompressor`].
    #[derive(Debug)]
    pub struct DictCompressor(lz4rip::block::DictCompressor);

    impl DictCompressor {
        /// Create a compressor for `dict`.
        pub fn new(dict: &[u8]) -> Self {
            Self(lz4rip::block::DictCompressor::new(dict))
        }

        /// Compress `input` into `output`.
        pub fn compress_into(
            &mut self,
            input: &[u8],
            output: &mut [u8],
        ) -> Result<usize, CompressError> {
            self.0.compress_into(input, output)
        }
    }

    /// [`lz4rip::block::Decompressor`].
    #[derive(Debug)]
    pub struct Decompressor(lz4rip::block::Decompressor);

    impl Decompressor {
        /// Create a decompressor for `dict`.
        pub fn with_dict(dict: &[u8]) -> Self {
            Self(lz4rip::block::Decompressor::with_dict(dict))
        }

        /// Decompress `input` into `output`.
        pub fn decompress_into(
            &self,
            input: &[u8],
            output: &mut [u8],
        ) -> Result<usize, DecompressError> {
            self.0.decompress_into(input, output)
        }
    }
}

/// lz4_flex block API, default features (safe).
pub mod lz4_flex {
    use lz4_flex_upstream::block::{CompressError, DecompressError};

    /// `lz4_flex::block::compress_into`.
    pub fn compress_into(input: &[u8], output: &mut [u8]) -> Result<usize, CompressError> {
        lz4_flex_upstream::block::compress_into(input, output)
    }

    /// `lz4_flex::block::decompress_into`.
    pub fn decompress_into(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
        lz4_flex_upstream::block::decompress_into(input, output)
    }
}

/// lz4_flex block API without its safe features.
pub mod lz4_flex_unsafe {
    use lz4_flex_unsafe::block::{CompressError, DecompressError};

    /// `lz4_flex::block::compress_into`.
    pub fn compress_into(input: &[u8], output: &mut [u8]) -> Result<usize, CompressError> {
        lz4_flex_unsafe::block::compress_into(input, output)
    }

    /// `lz4_flex::block::decompress_into`.
    pub fn decompress_into(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
        lz4_flex_unsafe::block::decompress_into(input, output)
    }
}
