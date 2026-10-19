//! LZ4 block decompression.

#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decompress;
mod out;
pub(crate) mod primitives;
mod validate;

#[cfg(feature = "alloc")]
pub use decompress::Decompressor;
pub use decompress::{DecompressorRef, decompress_into, decompress_into_with_dict};
#[cfg(feature = "alloc")]
pub use decompress::{decompress, decompress_append};
pub use lz4rip_core::DecompressError;
pub use validate::{BlockValidation, validate_block, validate_block_with_dict};

// Internal items needed by the lz4rip facade crate for the frame module.
#[doc(hidden)]
pub use decompress::{decompress_into_sink_with_dict, read_integer};
