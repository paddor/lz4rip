//! LZ4 Frame Format
//!
//! As defined in <https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md>
//!
//! # Example: compress data on `stdin` with frame format
//! This program reads data from `stdin`, compresses it and emits it to `stdout`.
//! This example can be found in `examples/compress.rs`:
//! ```no_run
//! use std::io;
//! let stdin = io::stdin();
//! let stdout = io::stdout();
//! let mut rdr = stdin.lock();
//! // Wrap the stdout writer in a LZ4 Frame writer.
//! let mut wtr = lz4rip::frame::FrameEncoder::new(stdout.lock());
//! io::copy(&mut rdr, &mut wtr).expect("I/O operation failed");
//! wtr.finish().unwrap();
//! ```
//!

use std::{fmt, io};

#[cfg(not(feature = "paranoid"))]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        unsafe { $e }
    };
}

#[cfg(feature = "paranoid")]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        $e
    };
}

pub(crate) mod compress;
#[forbid(unsafe_code)]
pub(crate) mod decompress;
#[forbid(unsafe_code)]
pub(crate) mod header;
#[forbid(unsafe_code)]
pub mod push;

pub use compress::{AutoFinishEncoder, FrameEncoder};
pub use decompress::FrameDecoder;
pub use header::{BlockMode, BlockSize, FrameInfo};

#[derive(Debug)]
#[non_exhaustive]
/// Errors that can occur when de/compressing lz4.
pub enum Error {
    /// Compression error.
    CompressionError(lz4rip_core::CompressError),
    /// Decompression error.
    DecompressionError(lz4rip_core::DecompressError),
    /// An io::Error was encountered.
    IoError(io::Error),
    /// Unsupported block size.
    UnsupportedBlocksize(u8),
    /// Unsupported frame version.
    UnsupportedVersion(u8),
    /// Wrong magic number for the LZ4 frame format.
    WrongMagicNumber,
    /// Reserved bits set.
    ReservedBitsSet,
    /// Block header is malformed.
    InvalidBlockInfo,
    /// Read a block larger than specified in the Frame header.
    BlockTooBig,
    /// The Frame header checksum doesn't match.
    HeaderChecksumError,
    /// The block checksum doesn't match.
    BlockChecksumError,
    /// The content checksum doesn't match.
    ContentChecksumError,
    /// Read an skippable frame.
    /// The caller may read the specified amount of bytes from the underlying io::Read.
    SkippableFrame(u32),
    /// External dictionaries are not supported.
    DictionaryNotSupported,
    /// The frame declares a Dict_ID but no dictionary was provided to the decoder.
    DictionaryRequired,
    /// Deprecated: dictionaries can use either independent or linked blocks.
    DictionaryRequiresIndependentBlocks,
    /// The frame's Dict_ID does not match the dictionary supplied to the decoder.
    DictIdMismatch {
        /// Dict_ID written into the frame header.
        expected: u32,
        /// Dict_ID of the dictionary provided to the decoder.
        actual: u32,
    },
    /// Content length differs.
    ContentLengthError {
        /// Expected content length.
        expected: u64,
        /// Actual content length.
        actual: u64,
    },
}

impl From<Error> for io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::IoError(e) => e,
            Error::CompressionError(_)
            | Error::DecompressionError(_)
            | Error::SkippableFrame(_)
            | Error::DictionaryNotSupported
            | Error::DictionaryRequired
            | Error::DictIdMismatch { .. } => io::Error::other(e),
            Error::DictionaryRequiresIndependentBlocks => {
                io::Error::new(io::ErrorKind::InvalidInput, e)
            }
            Error::WrongMagicNumber
            | Error::UnsupportedBlocksize(..)
            | Error::UnsupportedVersion(..)
            | Error::ReservedBitsSet
            | Error::InvalidBlockInfo
            | Error::BlockTooBig
            | Error::HeaderChecksumError
            | Error::ContentChecksumError
            | Error::BlockChecksumError
            | Error::ContentLengthError { .. } => io::Error::new(io::ErrorKind::InvalidData, e),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
            Some(_) => *e.into_inner().unwrap().downcast::<Error>().unwrap(),
            None => Error::IoError(e),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::CompressionError(e) => write!(f, "compression error: {e}"),
            Error::DecompressionError(e) => write!(f, "decompression error: {e}"),
            Error::IoError(e) => write!(f, "I/O error: {e}"),
            Error::UnsupportedBlocksize(b) => write!(f, "unsupported block size: {b}"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported frame version: {v}"),
            Error::WrongMagicNumber => f.write_str("wrong magic number"),
            Error::ReservedBitsSet => f.write_str("reserved bits set in frame descriptor"),
            Error::InvalidBlockInfo => f.write_str("invalid block header"),
            Error::BlockTooBig => f.write_str("block larger than frame header allows"),
            Error::HeaderChecksumError => f.write_str("frame header checksum mismatch"),
            Error::BlockChecksumError => f.write_str("block checksum mismatch"),
            Error::ContentChecksumError => f.write_str("content checksum mismatch"),
            Error::SkippableFrame(len) => write!(f, "skippable frame ({len} bytes)"),
            Error::DictionaryRequiresIndependentBlocks => {
                f.write_str("dictionary frames can use independent or linked blocks")
            }
            Error::DictionaryNotSupported => {
                f.write_str("frame declares a dictionary but no dictionary was provided")
            }
            Error::DictionaryRequired => {
                f.write_str("decoder has a dictionary but frame does not declare one")
            }
            Error::DictIdMismatch { expected, actual } => {
                write!(
                    f,
                    "dictionary ID mismatch: frame declares {expected:#010x}, decoder has {actual:#010x}"
                )
            }
            Error::ContentLengthError { expected, actual } => {
                write!(
                    f,
                    "content length mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}
