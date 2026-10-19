//! LZ4 block decompression.

#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(feature = "std"), no_std)]
// Decoding is safe code. The only `unsafe` is the uninitialized-output
// boundary: layout views between `[u8]` and `[MaybeUninit<u8>]` in `out` and
// the uninit entry points in `decompress`, each marked
// `#[allow(unsafe_code)]`. Paranoid and 32-bit builds keep one:
// `out::zero_init`.
#![deny(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

mod decompress;
mod out;
pub(crate) mod primitives;
mod validate;

#[cfg(feature = "alloc")]
pub use decompress::Decompressor;
pub use decompress::{
    DecompressorRef, decompress_into, decompress_into_uninit, decompress_into_with_dict,
};
#[cfg(feature = "alloc")]
pub use decompress::{decompress, decompress_append};
pub use lz4rip_core::DecompressError;
pub use validate::{BlockValidation, validate_block, validate_block_with_dict};

// Internal items needed by the lz4rip facade crate for the frame module.
#[doc(hidden)]
pub use decompress::{decompress_into_sink_with_dict, read_integer};
