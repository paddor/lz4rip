//! Shared types and constants for the lz4rip encode/decode crates.

#![forbid(unsafe_code)]
#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

use core::{error::Error, fmt};

mod sink;
pub use sink::{Sink, SliceSink};

mod fastcpy;
pub use fastcpy::slice_copy;

// --- LZ4 block format constants ---

/// Window size for LZ4 compression (64 KB).
pub const WINDOW_SIZE: usize = 64 * 1024;

/// Last match must start at least 12 bytes before end of block.
pub const MFLIMIT: usize = 12;

/// Last 5 bytes are always literals.
pub const LAST_LITERALS: usize = 5;

/// `LAST_LITERALS` + 1: extra byte for register-width reads near end.
pub const END_OFFSET: usize = LAST_LITERALS + 1;

/// Minimum compressible block length: MFLIMIT + 1 for the token.
pub const LZ4_MIN_LENGTH: usize = MFLIMIT + 1;

/// Maximum match distance (65535).
pub const MAX_DISTANCE: usize = (1 << 16) - 1;

/// Minimum match length.
pub const MINMATCH: usize = 4;

/// An error representing invalid compressed data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecompressError {
    /// The provided output is too small
    OutputTooSmall {
        /// Minimum expected output size
        expected: usize,
        /// Actual size of output
        actual: usize,
    },
    /// Literal is out of bounds of the input
    LiteralOutOfBounds,
    /// Expected another byte, but none found.
    ExpectedAnotherByte,
    /// Match offset is 0
    OffsetZero,
    /// Deduplication offset out of bounds (not in buffer).
    OffsetOutOfBounds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
/// Errors that can happen during compression.
pub enum CompressError {
    /// The provided output is too small.
    OutputTooSmall,
    /// The input or logical compression stream is too large for this target.
    InputTooLarge,
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DecompressError::OutputTooSmall { expected, actual } => {
                write!(
                    f,
                    "provided output is too small for the decompressed data, actual {actual}, expected \
                     {expected}"
                )
            }
            DecompressError::LiteralOutOfBounds => {
                f.write_str("literal is out of bounds of the input")
            }
            DecompressError::ExpectedAnotherByte => {
                f.write_str("expected another byte, found none")
            }
            DecompressError::OffsetZero => f.write_str("0 is not a valid match offset"),
            DecompressError::OffsetOutOfBounds => {
                f.write_str("the offset to copy is not contained in the decompressed buffer")
            }
        }
    }
}

impl fmt::Display for CompressError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CompressError::OutputTooSmall => f.write_str(
                "output is too small for the compressed data, use get_maximum_output_size to \
                 reserve enough space",
            ),
            CompressError::InputTooLarge => {
                f.write_str("input or compression stream is too large for this target")
            }
        }
    }
}

impl Error for DecompressError {}

impl Error for CompressError {}
