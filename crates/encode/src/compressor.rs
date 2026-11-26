//! Owning block compressors (alloc).

use core::fmt;

use alloc::vec::Vec;

use crate::compress::{
    CompressorRefN, DEFAULT_DICT_ENTRIES, DEFAULT_NODICT_ENTRIES, HashTableU32U16, RatioDictTables,
    RatioSmallTable, RatioTable, compress_dict_tables, compress_ratio_dict_tables,
    compress_ratio_into_sink_with_dict, compress_ratio_tables, finish_vec, get_maximum_output_size,
    init_dict,
};
use lz4rip_core::CompressError;
use lz4rip_core::SliceSink;
use lz4rip_core::{MINMATCH, WINDOW_SIZE};

/// A reusable no-dict block compressor (owning) with `N` hash-table entries.
///
/// [`Compressor`] is the standard-sized alias (8 KB table). Use this generic
/// form to pick a smaller table, e.g. `CompressorN::<512>::new()` for a 2 KB
/// table. `N` must be a power of two (checked at compile time).
///
/// This is the ergonomic owning API for use with `alloc`. For a no-alloc
/// variant, see [`CompressorRef`](crate::CompressorRef). For a dictionary-seeded
/// compressor, see [`DictCompressor`].
///
/// For one-shot compression, use [`compress`](crate::compress) or
/// [`compress_into`](crate::compress_into) instead.
///
/// # Example
/// ```
/// use lz4rip_encode::{Compressor, get_maximum_output_size};
///
/// let mut comp = Compressor::new();
/// let input = b"hello world, hello world, hello!";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct CompressorN<const N: usize = DEFAULT_NODICT_ENTRIES> {
    inner: CompressorRefN<N>,
}

/// A reusable no-dict block compressor (owning) with the standard 8 KB table.
pub type Compressor = CompressorN<DEFAULT_NODICT_ENTRIES>;

impl<const N: usize> CompressorN<N> {
    /// Create a new compressor without a dictionary.
    #[must_use]
    pub fn new() -> Self {
        CompressorN {
            inner: CompressorRefN::<N>::new(),
        }
    }

    /// Bytes this compressor holds on the heap: its table (`4 * N`).
    pub fn heap_size(&self) -> usize {
        self.inner.heap_size()
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`](crate::get_maximum_output_size)`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        self.inner.compress_into(input, output)
    }

    /// Compress `input` into a new `Vec<u8>`.
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        self.inner.compress(input)
    }
}

/// A reusable dict block compressor (owning) with `N` entries per table.
///
/// [`DictCompressor`] is the standard-sized alias (two 8 KB tables). Use this
/// generic form to pick smaller tables, e.g. `DictCompressorN::<1024>::new(dict)`
/// for two 2 KB tables. `N` must be a power of two (checked at compile time).
///
/// This is the ergonomic owning dict API for use with `alloc`. For a no-alloc
/// variant that borrows the dictionary, see
/// [`DictCompressorRef`](crate::DictCompressorRef).
///
/// Unlike the previous self-referential design, this owns its hash tables and
/// dictionary as sibling fields and needs no `unsafe`.
///
/// # Example
/// ```
/// use lz4rip_encode::{DictCompressor, get_maximum_output_size};
///
/// let dict = b"the quick brown fox";
/// let mut comp = DictCompressor::new(dict);
/// let input = b"the quick brown fox jumps";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
pub struct DictCompressorN<const N: usize = DEFAULT_DICT_ENTRIES> {
    /// Trimmed dictionary bytes (empty when the dictionary was shorter than
    /// [`MINMATCH`]).
    dict: Vec<u8>,
    table: HashTableU32U16<N>,
    pristine: HashTableU32U16<N>,
    main_base: usize,
}

/// A reusable dict block compressor (owning) with the standard 8 KB tables.
pub type DictCompressor = DictCompressorN<DEFAULT_DICT_ENTRIES>;

impl<const N: usize> fmt::Debug for DictCompressorN<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DictCompressor")
            .field("dict_len", &self.dict.len())
            .finish()
    }
}

impl<const N: usize> DictCompressorN<N> {
    /// Create a new compressor seeded with an external dictionary.
    ///
    /// The dictionary is cloned into owned storage. If `dict` is longer than the
    /// LZ4 window it is trimmed to the last [`WINDOW_SIZE`] bytes. A dictionary
    /// shorter than 4 bytes is ignored (no dict matches); use [`Compressor`] for
    /// that case.
    #[must_use]
    pub fn new(dict: &[u8]) -> Self {
        let trimmed = if dict.len() < MINMATCH {
            b"".as_slice()
        } else if dict.len() > WINDOW_SIZE {
            &dict[dict.len() - WINDOW_SIZE..]
        } else {
            dict
        };
        let mut pristine = HashTableU32U16::<N>::new();
        let mut dict_ref = trimmed;
        init_dict(&mut pristine, &mut dict_ref);
        DictCompressorN {
            dict: trimmed.to_vec(),
            table: HashTableU32U16::<N>::new(),
            pristine,
            main_base: 1,
        }
    }

    /// Bytes this compressor holds on the heap: its two tables (`2 * N`
    /// each) and its copy of the dictionary.
    pub fn heap_size(&self) -> usize {
        self.table.heap_size() + self.pristine.heap_size() + self.dict.capacity()
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`](crate::get_maximum_output_size)`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        // Split the borrow so the dict and tables can be passed separately.
        let DictCompressorN {
            dict,
            table,
            pristine,
            main_base,
        } = self;
        compress_dict_tables(table, pristine, main_base, dict, input, output)
    }

    /// Compress `input` into a new `Vec<u8>`.
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        let max = get_maximum_output_size(input.len());
        let mut out = alloc::vec![0u8; max];
        let n = self.compress_into(input, &mut out).unwrap();
        finish_vec(out, max, n)
    }
}

/// A reusable no-dict block compressor with the ratio profile.
///
/// Output equals [`compress_ratio_into`](crate::compress_ratio_into)'s:
/// smaller than [`Compressor`]'s, at slower compression. It owns an 8 KB
/// table, and a 64 KB one from its first input above 4 KiB. Inputs up to
/// 256 B reuse the small one across calls; larger inputs clear a table per
/// call.
///
/// # Example
/// ```
/// use lz4rip_encode::{RatioCompressor, get_maximum_output_size};
///
/// let mut comp = RatioCompressor::new();
/// let input = b"hello world, hello world, hello!";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
pub struct RatioCompressor {
    small: RatioSmallTable,
    /// Where the next small input's positions start in `small`.
    small_offset: usize,
    /// Made by the first input above 4 KiB.
    large: Option<RatioTable>,
}

impl fmt::Debug for RatioCompressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RatioCompressor").finish_non_exhaustive()
    }
}

impl Default for RatioCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl RatioCompressor {
    /// Create a new compressor.
    #[must_use]
    pub fn new() -> Self {
        RatioCompressor {
            small: RatioSmallTable::new(),
            small_offset: 1,
            large: None,
        }
    }

    /// Bytes this compressor holds on the heap: its 8 KB table, and the
    /// 64 KB one once an input above 4 KiB made it.
    pub fn heap_size(&self) -> usize {
        self.small.heap_size() + self.large.as_ref().map_or(0, RatioTable::heap_size)
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`](crate::get_maximum_output_size)`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        compress_ratio_tables(
            &mut self.small,
            &mut self.small_offset,
            &mut self.large,
            input,
            output,
        )
    }

    /// Compress `input` into a new `Vec<u8>`.
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        let max = get_maximum_output_size(input.len());
        let mut out = alloc::vec![0u8; max];
        let n = self.compress_into(input, &mut out).unwrap();
        finish_vec(out, max, n)
    }
}

/// A reusable block compressor with the ratio profile and a dictionary.
///
/// Output equals the ratio profile's with the same dictionary hashed for each
/// input, as a frame encoder with a dictionary compresses a block. This
/// compressor hashes the dictionary once per size class of
/// dictionary + input and copies that table for each input instead: 8 KB up to
/// 4 KiB, 32 KB below 64 KiB, and 64 KB above. A class's tables are made by
/// its first input, two per class. Decode with the same dictionary, e.g.
/// [`decompress_into_with_dict`](https://docs.rs/lz4rip/latest/lz4rip/block/fn.decompress_into_with_dict.html).
///
/// A dictionary longer than the LZ4 window is trimmed to its last
/// [`WINDOW_SIZE`] bytes. One shorter than 4 bytes is ignored: the output is
/// then [`RatioCompressor`]'s, and each call makes its own table.
///
/// # Example
/// ```
/// use lz4rip_encode::{RatioDictCompressor, get_maximum_output_size};
///
/// let dict = b"hello world, hello rust, hello lz4";
/// let mut comp = RatioDictCompressor::new(dict);
/// let input = b"hello world, hello lz4!";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
pub struct RatioDictCompressor {
    dict: Vec<u8>,
    tables: RatioDictTables,
}

impl fmt::Debug for RatioDictCompressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RatioDictCompressor")
            .field("dict_len", &self.dict.len())
            .finish_non_exhaustive()
    }
}

impl RatioDictCompressor {
    /// Create a new compressor with a copy of `dict`. Its tables are made by
    /// the first inputs that need them.
    #[must_use]
    pub fn new(dict: &[u8]) -> Self {
        let trimmed = if dict.len() < MINMATCH {
            b"".as_slice()
        } else if dict.len() > WINDOW_SIZE {
            &dict[dict.len() - WINDOW_SIZE..]
        } else {
            dict
        };
        RatioDictCompressor {
            dict: trimmed.to_vec(),
            tables: RatioDictTables::default(),
        }
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`](crate::get_maximum_output_size)`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        if self.dict.is_empty() {
            return compress_ratio_into_sink_with_dict::<false>(
                input,
                &mut SliceSink::new(output, 0),
                b"",
            );
        }
        compress_ratio_dict_tables(&mut self.tables, &self.dict, input, output)
    }

    /// Compress `input` into a new `Vec<u8>`.
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        let max = get_maximum_output_size(input.len());
        let mut out = alloc::vec![0u8; max];
        let n = self.compress_into(input, &mut out).unwrap();
        finish_vec(out, max, n)
    }

    /// Bytes this compressor holds on the heap: its copy of the dictionary,
    /// and the tables its inputs have made so far.
    pub fn heap_size(&self) -> usize {
        self.dict.capacity() + self.tables.heap_size()
    }
}
