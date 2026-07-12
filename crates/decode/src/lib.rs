//! LZ4 block decompression.

#![forbid(unsafe_code)]
#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", allow(internal_features))]
#![cfg_attr(feature = "nightly", feature(core_intrinsics))]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decompress;
pub(crate) mod primitives;

#[cfg(feature = "alloc")]
pub use decompress::Decompressor;
#[cfg(feature = "alloc")]
pub use decompress::decompress;
pub use decompress::{DecompressorRef, decompress_into, decompress_into_with_dict};
pub use lz4rip_core::DecompressError;

// Internal items needed by the lz4rip facade crate for the frame module.
#[doc(hidden)]
pub use decompress::{decompress_into_sink_with_dict, read_integer};
