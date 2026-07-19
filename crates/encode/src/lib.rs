//! LZ4 block compression.

#![deny(warnings)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]
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

mod compress;
#[cfg(feature = "alloc")]
mod compressor;
#[cfg(feature = "alloc")]
#[forbid(unsafe_code)]
mod dict;
pub(crate) mod hashtable;
mod verified_sink;

#[cfg(feature = "alloc")]
pub use compress::compress;
pub use compress::{
    CompressorRef, CompressorRefN, DEFAULT_DICT_ENTRIES, DEFAULT_NODICT_ENTRIES, DictCompressorRef,
    DictCompressorRefN, MIN_ENTRIES, compress_into, compress_into_with_dict,
    get_maximum_output_size,
};
#[cfg(feature = "alloc")]
pub use compressor::{Compressor, CompressorN, DictCompressor, DictCompressorN};
#[cfg(feature = "alloc")]
pub use dict::DictTrainer;
pub use lz4rip_core::CompressError;

// Cross-crate plumbing for the lz4rip facade (frame module + tests).
// Public for workspace access but not part of the stable API.
#[doc(hidden)]
pub use compress::{compress_into_sink_with_dict, seed_table_with_input, write_integer};
#[doc(hidden)]
pub use hashtable::HashTableU32;

/// Compress with a caller-owned `HashTableU32`.
///
/// This is cross-crate plumbing for the frame encoder.
///
/// # Safety
///
/// The caller must ensure `table` is owned by the logical compression stream
/// described by `input`, `input_pos`, `ext_dict`, and `input_stream_offset`.
/// Every live table entry that can be accepted as an input or dictionary
/// candidate must map to at least four readable bytes in that source slice.
/// In practice, callers should only pass a table initialized and maintained by
/// this function, optionally seeded by `seed_table_with_input` using bytes from
/// the same logical stream.
#[doc(hidden)]
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
pub unsafe fn compress_into_sink_with_table<
    const USE_DICT: bool,
    const HAS_OFFSET: bool,
    const READONLY: bool,
    S: lz4rip_core::Sink,
>(
    input: &[u8],
    input_pos: usize,
    output: &mut S,
    table: &mut hashtable::HashTableU32,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, lz4rip_core::CompressError> {
    compress::compress_into_sink_with_table_inner::<USE_DICT, HAS_OFFSET, READONLY, _>(
        input,
        input_pos,
        output,
        table,
        ext_dict,
        input_stream_offset,
    )
}

/// Compress with a caller-owned `HashTableU32`.
///
/// This is cross-crate plumbing for the frame encoder. In the paranoid build
/// this is safe because the implementation uses bounds-checked memory accesses.
#[doc(hidden)]
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
pub fn compress_into_sink_with_table<
    const USE_DICT: bool,
    const HAS_OFFSET: bool,
    const READONLY: bool,
    S: lz4rip_core::Sink,
>(
    input: &[u8],
    input_pos: usize,
    output: &mut S,
    table: &mut hashtable::HashTableU32,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, lz4rip_core::CompressError> {
    compress::compress_into_sink_with_table_inner::<USE_DICT, HAS_OFFSET, READONLY, _>(
        input,
        input_pos,
        output,
        table,
        ext_dict,
        input_stream_offset,
    )
}
