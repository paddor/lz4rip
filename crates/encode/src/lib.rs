//! LZ4 block compression.

#![forbid(unsafe_code)]
#![deny(warnings)]
#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(feature = "nightly", feature(optimize_attribute))]

#[cfg(feature = "alloc")]
extern crate alloc;

mod compress;
#[cfg(feature = "alloc")]
mod compressor;
#[cfg(feature = "alloc")]
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
#[doc(hidden)]
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
