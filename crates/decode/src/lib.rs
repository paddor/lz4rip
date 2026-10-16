//! LZ4 block decompression.

#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", allow(internal_features))]
#![cfg_attr(feature = "nightly", feature(core_intrinsics))]
// `optimize_attribute` is stable since 1.101. The gate keeps older nightlies
// working.
#![cfg_attr(feature = "nightly", allow(stable_features))]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]
// Paranoid and 32-bit builds deny unsafe code. The one exception,
// `out::zero_init`, views a zero-filled uninitialized buffer as bytes.
#![cfg_attr(
    any(feature = "paranoid", target_pointer_width = "32"),
    deny(unsafe_code)
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
