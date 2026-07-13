//! Building blocks for push-based frame decoding.
//!
//! The [`FrameDecoder`](super::FrameDecoder) is pull-based (`io::Read`).
//! These primitives let callers build a push-based decoder by parsing frame
//! and block headers from byte slices, then decompressing each block with the
//! [block API](crate::block).
//!
//! # Typical flow
//!
//! 1. Buffer at least [`MIN_HEADER_SIZE`] bytes.
//! 2. Call [`frame_header_size`] to learn the exact header length.
//! 3. Buffer until that many bytes are available, then [`parse_frame_header`].
//! 4. Read the next [`BLOCK_HEADER_SIZE`] bytes and [`parse_block_header`].
//!    - [`BlockHeader::EndMark`]: the frame is complete.
//!    - [`BlockHeader::Compressed(n)`]: read `n` bytes and decompress with
//!      [`block::decompress_into`](crate::block::decompress_into).
//!    - [`BlockHeader::Uncompressed(n)`]: read `n` bytes verbatim.
//! 5. If [`FrameInfo::block_checksums`] is set, read 4 bytes after each block
//!    (xxHash32, seed 0, of the raw block data).
//! 6. Repeat from step 4.
//! 7. After [`BlockHeader::EndMark`], if [`FrameInfo::content_checksum`] is set,
//!    read 4 bytes (xxHash32, seed 0, of all uncompressed content).

use super::Error;
use super::header::{BlockInfo, FrameInfo};

/// Minimum bytes needed to determine the full frame header size.
pub const MIN_HEADER_SIZE: usize = super::header::MIN_FRAME_INFO_SIZE;

/// Maximum possible frame header size.
pub const MAX_HEADER_SIZE: usize = super::header::MAX_FRAME_INFO_SIZE;

/// Size of a block header in bytes.
pub const BLOCK_HEADER_SIZE: usize = super::header::BLOCK_INFO_SIZE;

/// A parsed block header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockHeader {
    /// Compressed block. The value is the size of the compressed data in bytes.
    Compressed(u32),
    /// Uncompressed (stored) block. The value is the size of the raw data in bytes.
    Uncompressed(u32),
    /// End-of-frame marker.
    EndMark,
}

/// Returns how many bytes the frame header in `input` requires.
///
/// `input` must be at least [`MIN_HEADER_SIZE`] bytes. If the header includes
/// optional fields (content size, dictionary ID), the returned value will be
/// larger than `MIN_HEADER_SIZE`.
pub fn frame_header_size(input: &[u8]) -> Result<usize, Error> {
    FrameInfo::read_size(input)
}

/// Parse a frame header from `input`.
///
/// `input` must contain at least the number of bytes returned by
/// [`frame_header_size`].
pub fn parse_frame_header(input: &[u8]) -> Result<FrameInfo, Error> {
    FrameInfo::read(input)
}

/// Parse a 4-byte block header.
pub fn parse_block_header(input: &[u8; 4]) -> Result<BlockHeader, Error> {
    match BlockInfo::read(input)? {
        BlockInfo::Compressed(len) => Ok(BlockHeader::Compressed(len)),
        BlockInfo::Uncompressed(len) => Ok(BlockHeader::Uncompressed(len)),
        BlockInfo::EndMark => Ok(BlockHeader::EndMark),
    }
}
