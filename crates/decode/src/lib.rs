//! LZ4 block decompression.

#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", allow(internal_features))]
#![cfg_attr(feature = "nightly", feature(core_intrinsics))]
#![cfg_attr(
    any(feature = "paranoid", target_pointer_width = "32"),
    forbid(unsafe_code)
)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        unsafe { $e }
    };
}

#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
macro_rules! paranoid_unsafe_call {
    ($e:expr) => {
        $e
    };
}

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
