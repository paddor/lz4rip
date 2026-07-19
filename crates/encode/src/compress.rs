//! LZ4 block compression.

use core::fmt;

use crate::hashtable::HashTable;
use crate::verified_sink::VerifiedSliceSink;
#[cfg(feature = "alloc")]
use alloc::vec;
use lz4rip_core::CompressError;
use lz4rip_core::END_OFFSET;
use lz4rip_core::LZ4_MIN_LENGTH;
use lz4rip_core::MAX_DISTANCE;
use lz4rip_core::MFLIMIT;
use lz4rip_core::MINMATCH;
use lz4rip_core::Sink;
use lz4rip_core::WINDOW_SIZE;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

pub(crate) use crate::hashtable::HashTableU32;
pub(crate) use crate::hashtable::HashTableU32U16;
pub use crate::hashtable::{DEFAULT_DICT_ENTRIES, DEFAULT_NODICT_ENTRIES, MIN_ENTRIES};

/// Inputs up to this size reuse the no-dict hash table across calls (epoch-based
/// table reuse); larger inputs clear it. Independent of table entry count.
const EPOCH_THRESHOLD: usize = 8 * 1024;

/// Skip acceleration: step grows by 1 every `1 << N` consecutive non-matches.
/// C lz4 uses 6; see DESIGN.md for tradeoff analysis.
const INCREASE_STEPSIZE_BITSHIFT: usize = 3;

/// Inputs up to this size use the dict table read-only (no per-call clearing or
/// table writes). Self-references within small inputs are rare; the dict provides
/// virtually all matches. Skips the 8 KB table.clear() and all put_at writes.
const DICT_READONLY_LIMIT: usize = 256;

#[cfg(target_pointer_width = "32")]
#[inline]
fn ensure_stream_bounds(
    input_stream_offset: usize,
    input_len: usize,
    ext_dict_len: usize,
) -> Result<(), CompressError> {
    if ext_dict_len > input_stream_offset {
        return Err(CompressError::InputTooLarge);
    }
    let Some(span) = input_stream_offset
        .checked_add(input_len)
        .and_then(|i| i.checked_add(ext_dict_len))
    else {
        return Err(CompressError::InputTooLarge);
    };
    if span > isize::MAX as usize {
        return Err(CompressError::InputTooLarge);
    }
    Ok(())
}

#[cfg(not(target_pointer_width = "32"))]
#[inline]
fn assert_stream_bounds(input_stream_offset: usize, input_len: usize, ext_dict_len: usize) {
    assert!(ext_dict_len <= input_stream_offset);
    assert!(
        input_stream_offset
            .checked_add(input_len)
            .and_then(|i| i.checked_add(ext_dict_len))
            .is_some_and(|i| i <= isize::MAX as usize)
    );
}

#[inline]
fn token_from_literal(lit_len: usize) -> u8 {
    if lit_len < 0xF {
        (lit_len as u8) << 4
    } else {
        0xF0
    }
}

#[inline]
fn token_from_literal_and_match_length(lit_len: usize, duplicate_length: usize) -> u8 {
    let mut token = if lit_len < 0xF {
        (lit_len as u8) << 4
    } else {
        0xF0
    };

    token |= if duplicate_length < 0xF {
        duplicate_length as u8
    } else {
        0xF
    };

    token
}

/// Write a variable-length integer in the LZ4 encoding.
#[inline]
pub fn write_integer(output: &mut impl Sink, mut n: usize) {
    while n >= 0xFF {
        n -= 0xFF;
        push_byte(output, 0xFF);
    }
    push_byte(output, n as u8);
}

#[cold]
fn handle_last_literals(output: &mut impl Sink, input: &[u8], start: usize) {
    let lit_len = input.len() - start;

    let token = token_from_literal(lit_len);
    push_byte(output, token);
    if lit_len >= 0xF {
        write_integer(output, lit_len - 0xF);
    }
    output.extend_from_slice(&input[start..]);
}

#[inline]
fn backtrack_match(
    input: &[u8],
    cur: &mut usize,
    literal_start: usize,
    source: &[u8],
    candidate: &mut usize,
) {
    while *candidate > 0 && *cur > literal_start && input[*cur - 1] == source[*candidate - 1] {
        *cur -= 1;
        *candidate -= 1;
    }
}

/// Core block compression loop, monomorphized over hash table type and dict mode.
#[inline(never)]
pub(crate) fn compress_internal<
    T: HashTable,
    const USE_DICT: bool,
    const HAS_OFFSET: bool,
    const READONLY: bool,
    S: Sink,
>(
    input: &[u8],
    input_pos: usize,
    output: &mut S,
    table: &mut T,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, CompressError> {
    assert!(input_pos <= input.len());
    if USE_DICT {
        assert!(ext_dict.len() <= WINDOW_SIZE);
        #[cfg(target_pointer_width = "32")]
        ensure_stream_bounds(input_stream_offset, input.len(), ext_dict.len())?;
        #[cfg(not(target_pointer_width = "32"))]
        assert_stream_bounds(input_stream_offset, input.len(), ext_dict.len());
    } else {
        assert!(ext_dict.is_empty());
    }
    if !HAS_OFFSET {
        debug_assert_eq!(input_stream_offset, 0);
    }
    let input_stream_offset = if HAS_OFFSET { input_stream_offset } else { 0 };
    if output.capacity() - output.pos() < get_maximum_output_size(input.len() - input_pos) {
        return Err(CompressError::OutputTooSmall);
    }

    let output_start_pos = output.pos();
    if input.len() - input_pos < LZ4_MIN_LENGTH {
        handle_last_literals(output, input, input_pos);
        return Ok(output.pos() - output_start_pos);
    }

    let ext_dict_stream_offset = input_stream_offset - ext_dict.len();
    let end_pos_check = input.len() - MFLIMIT;
    let mut literal_start = input_pos;
    let mut cur = input_pos;

    if cur == 0 && input_stream_offset == 0 {
        let hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, 0));
        if !READONLY {
            table.put_at(hash, 0);
        }
        cur = 1;
    }

    let mut forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur));

    loop {
        let mut candidate;
        let mut candidate_source;
        let mut offset;
        let mut non_match_count = 1 << INCREASE_STEPSIZE_BITSHIFT;

        loop {
            let step = non_match_count >> INCREASE_STEPSIZE_BITSHIFT;
            non_match_count += 1;
            let next_cur = cur + step;

            if next_cur > end_pos_check + 1 {
                handle_last_literals(output, input, literal_start);
                return Ok(output.pos() - output_start_pos);
            }

            let hash = forward_hash;
            candidate = table.get_at(hash);
            forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, next_cur));
            if !READONLY {
                table.put_at(hash, cur + input_stream_offset);
            }

            debug_assert!(READONLY || candidate <= input_stream_offset + cur);

            if candidate >= input_stream_offset
                && input_stream_offset + cur - candidate <= MAX_DISTANCE
            {
                offset = (input_stream_offset + cur - candidate) as u16;
                candidate -= input_stream_offset;
                candidate_source = input;
            } else if USE_DICT
                && candidate >= ext_dict_stream_offset
                && input_stream_offset + cur - candidate <= MAX_DISTANCE
            {
                offset = (input_stream_offset + cur - candidate) as u16;
                candidate -= ext_dict_stream_offset;
                candidate_source = ext_dict;
            } else {
                cur = next_cur;
                continue;
            }
            let cand_bytes: u32 = paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(
                candidate_source,
                candidate
            ));
            let curr_bytes: u32 =
                paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(input, cur));

            if cand_bytes == curr_bytes {
                break;
            }
            cur = next_cur;
        }

        loop {
            backtrack_match(
                input,
                &mut cur,
                literal_start,
                candidate_source,
                &mut candidate,
            );

            let lit_len = cur - literal_start;

            cur += MINMATCH;
            candidate += MINMATCH;
            let duplicate_length =
                paranoid_unsafe_call!(crate::hashtable::count_same_bytes_inbounds(
                    input,
                    &mut cur,
                    candidate_source,
                    candidate,
                    END_OFFSET,
                ));

            let hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur - 2));
            if !READONLY {
                table.put_at(hash, cur - 2 + input_stream_offset);
            }

            let token = token_from_literal_and_match_length(lit_len, duplicate_length);
            push_byte(output, token);
            if lit_len >= 0xF {
                write_integer(output, lit_len - 0xF);
            }
            if lit_len > 0 {
                copy_literals_wild(output, input, literal_start, lit_len);
            }
            push_u16(output, offset);
            if duplicate_length >= 0xF {
                write_integer(output, duplicate_length - 0xF);
            }
            literal_start = cur;

            if !USE_DICT && cur <= end_pos_check {
                let hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur));
                let rematch = table.get_at(hash);

                if input_stream_offset + cur - rematch <= MAX_DISTANCE
                    && rematch >= input_stream_offset
                {
                    let rc = rematch - input_stream_offset;
                    if paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(input, cur))
                        == paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(input, rc))
                    {
                        table.put_at(hash, cur + input_stream_offset);
                        candidate = rc;
                        candidate_source = input;
                        offset = (input_stream_offset + cur - rematch) as u16;
                        continue;
                    }
                }
                forward_hash = hash;
            } else if cur <= end_pos_check {
                forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur));
            }
            break;
        }
    }
}

/// Compress with a caller-owned `HashTableU32`.
///
/// This is cross-crate plumbing for the frame encoder. It keeps the internal
/// `HashTable` trait private, so downstream safe code cannot corrupt match
/// finder invariants that protect unchecked reads.
pub(crate) fn compress_into_sink_with_table_inner<
    const USE_DICT: bool,
    const HAS_OFFSET: bool,
    const READONLY: bool,
    S: Sink,
>(
    input: &[u8],
    input_pos: usize,
    output: &mut S,
    table: &mut HashTableU32,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, CompressError> {
    compress_internal::<_, USE_DICT, HAS_OFFSET, READONLY, _>(
        input,
        input_pos,
        output,
        table,
        ext_dict,
        input_stream_offset,
    )
}

/// Seed a caller-owned `HashTableU32` from uncompressed input bytes.
///
/// This is cross-crate plumbing for the frame encoder. Dictionary-compressed
/// linked frames compress the first block with the normal dictionary path, then
/// seed the reusable linked-block table from that decoded block so later blocks
/// can reference it.
pub fn seed_table_with_input(table: &mut HashTableU32, input: &[u8], input_stream_offset: usize) {
    let mut i = 0usize;
    while i + core::mem::size_of::<usize>() <= input.len() {
        let hash = <HashTableU32 as HashTable>::get_hash_at(input, i);
        table.put_at(hash, i + input_stream_offset);
        i += 3;
    }
}

/// Dual-table compression for `CompressorRef::with_dict`.
#[inline(never)]
fn compress_with_dict_table<T: HashTable, S: Sink>(
    input: &[u8],
    output: &mut S,
    table: &mut T,
    dict_table: &T,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, CompressError> {
    debug_assert_eq!(input_stream_offset, ext_dict.len());
    assert!(ext_dict.len() <= WINDOW_SIZE);
    #[cfg(target_pointer_width = "32")]
    ensure_stream_bounds(input_stream_offset, input.len(), ext_dict.len())?;
    #[cfg(not(target_pointer_width = "32"))]
    assert_stream_bounds(input_stream_offset, input.len(), ext_dict.len());
    if output.capacity() - output.pos() < get_maximum_output_size(input.len()) {
        return Err(CompressError::OutputTooSmall);
    }

    let output_start_pos = output.pos();
    if input.len() < LZ4_MIN_LENGTH {
        handle_last_literals(output, input, 0);
        return Ok(output.pos() - output_start_pos);
    }

    let end_pos_check = input.len() - MFLIMIT;
    let mut literal_start = 0;

    let hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, 0));
    table.put_at(hash, input_stream_offset);
    let mut cur = 1;

    let mut forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur));

    loop {
        let mut candidate;
        let candidate_source;
        let offset;
        let mut non_match_count = 1 << INCREASE_STEPSIZE_BITSHIFT;

        loop {
            let step = non_match_count >> INCREASE_STEPSIZE_BITSHIFT;
            non_match_count += 1;
            let next_cur = cur + step;

            if next_cur > end_pos_check + 1 {
                handle_last_literals(output, input, literal_start);
                return Ok(output.pos() - output_start_pos);
            }

            let hash = forward_hash;
            forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, next_cur));
            let curr_bytes: u32 =
                paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(input, cur));

            let main_candidate = table.get_at(hash);
            table.put_at(hash, cur + input_stream_offset);

            // Probe dict table first: for small inputs most matches come from
            // the dict, and even for large inputs the dict covers the first
            // window of repeated structure.
            let dict_candidate = dict_table.get_at(hash);
            if dict_candidate < input_stream_offset
                && input_stream_offset + cur - dict_candidate <= MAX_DISTANCE
            {
                let cand_bytes: u32 = paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(
                    ext_dict,
                    dict_candidate
                ));
                if cand_bytes == curr_bytes {
                    offset = (input_stream_offset + cur - dict_candidate) as u16;
                    candidate = dict_candidate;
                    candidate_source = ext_dict;
                    break;
                }
            }

            if main_candidate >= input_stream_offset
                && input_stream_offset + cur - main_candidate <= MAX_DISTANCE
            {
                let cand_bytes: u32 = paranoid_unsafe_call!(crate::hashtable::get_batch_inbounds(
                    input,
                    main_candidate - input_stream_offset,
                ));
                if cand_bytes == curr_bytes {
                    offset = (input_stream_offset + cur - main_candidate) as u16;
                    candidate = main_candidate - input_stream_offset;
                    candidate_source = input;
                    break;
                }
            }

            cur = next_cur;
        }

        backtrack_match(
            input,
            &mut cur,
            literal_start,
            candidate_source,
            &mut candidate,
        );

        let lit_len = cur - literal_start;

        cur += MINMATCH;
        candidate += MINMATCH;
        let duplicate_length = paranoid_unsafe_call!(crate::hashtable::count_same_bytes_inbounds(
            input,
            &mut cur,
            candidate_source,
            candidate,
            END_OFFSET,
        ));

        let hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur - 2));
        table.put_at(hash, cur - 2 + input_stream_offset);

        let token = token_from_literal_and_match_length(lit_len, duplicate_length);
        push_byte(output, token);
        if lit_len >= 0xF {
            write_integer(output, lit_len - 0xF);
        }
        if lit_len > 0 {
            copy_literals_wild(output, input, literal_start, lit_len);
        }
        push_u16(output, offset);
        if duplicate_length >= 0xF {
            write_integer(output, duplicate_length - 0xF);
        }
        literal_start = cur;

        if cur <= end_pos_check {
            forward_hash = paranoid_unsafe_call!(T::get_hash_at_inbounds(input, cur));
        }
    }
}

#[inline]
fn push_byte(output: &mut impl Sink, el: u8) {
    output.push(el);
}

#[inline]
fn push_u16(output: &mut impl Sink, el: u16) {
    output.extend_from_slice(&el.to_le_bytes());
}

#[inline(always)]
fn copy_literals_wild(output: &mut impl Sink, input: &[u8], input_start: usize, len: usize) {
    output.extend_from_slice_wild(&input[input_start..input_start + len], len)
}

/// Compress `input` into `output` with optional dictionary data.
pub fn compress_into_sink_with_dict<const USE_DICT: bool>(
    input: &[u8],
    output: &mut impl Sink,
    mut dict_data: &[u8],
) -> Result<usize, CompressError> {
    if USE_DICT && dict_data.len() < MINMATCH {
        return compress_into_sink_with_dict::<false>(input, output, b"");
    }
    if dict_data.len() + input.len() < u16::MAX as usize {
        let mut dict: HashTableU32U16 = HashTableU32U16::new();
        init_dict(&mut dict, &mut dict_data);
        compress_internal::<_, USE_DICT, USE_DICT, false, _>(
            input,
            0,
            output,
            &mut dict,
            dict_data,
            dict_data.len(),
        )
    } else {
        let mut dict: HashTableU32 = HashTableU32::new();
        init_dict(&mut dict, &mut dict_data);
        compress_internal::<_, USE_DICT, USE_DICT, false, _>(
            input,
            0,
            output,
            &mut dict,
            dict_data,
            dict_data.len(),
        )
    }
}

#[inline]
pub(crate) fn init_dict<T: HashTable>(dict: &mut T, dict_data: &mut &[u8]) {
    if dict_data.len() > WINDOW_SIZE {
        *dict_data = &dict_data[dict_data.len() - WINDOW_SIZE..];
    }
    let mut i = 0usize;
    while i + core::mem::size_of::<usize>() <= dict_data.len() {
        let hash = T::get_hash_at(dict_data, i);
        dict.put_at(hash, i);
        i += 3;
    }
}

/// Returns the maximum output size of the compressed data.
/// Can be used to preallocate capacity on the output vector.
///
/// Returns `usize::MAX` if the result would overflow (e.g. on 32-bit with >3.9 GB input).
#[must_use]
#[inline]
pub const fn get_maximum_output_size(input_len: usize) -> usize {
    let raw = 16u64 + 4 + (input_len as u64 * 110 / 100);
    if raw > usize::MAX as u64 {
        usize::MAX
    } else {
        raw as usize
    }
}

/// Compress all bytes of `input` into `output`.
/// output should be preallocated with a size of
/// `get_maximum_output_size`.
///
/// Returns the number of bytes written (compressed) into `output`.
#[inline]
pub fn compress_into(input: &[u8], output: &mut [u8]) -> Result<usize, CompressError> {
    compress_into_sink_with_dict::<false>(input, &mut VerifiedSliceSink::new(output, 0), b"")
}

/// Compress all bytes of `input` into `output` using an external dictionary.
///
/// Returns the number of bytes written (compressed) into `output`.
#[inline]
pub fn compress_into_with_dict(
    input: &[u8],
    output: &mut [u8],
    dict: &[u8],
) -> Result<usize, CompressError> {
    compress_into_sink_with_dict::<true>(input, &mut VerifiedSliceSink::new(output, 0), dict)
}

/// Compress all bytes of `input`.
#[must_use]
#[cfg(feature = "alloc")]
#[inline]
pub fn compress(input: &[u8]) -> Vec<u8> {
    let max_compressed_size = get_maximum_output_size(input.len());
    let mut compressed: Vec<u8> = vec![0u8; max_compressed_size];
    let compressed_len = compress_into_sink_with_dict::<false>(
        input,
        &mut VerifiedSliceSink::new(&mut compressed, 0),
        b"",
    )
    .unwrap();
    compressed.truncate(compressed_len);

    compressed
}

/// A reusable no-dict block compressor with `N` hash-table entries.
///
/// [`CompressorRef`] is the standard-sized alias (8 KB table). Use this generic
/// form to pick a smaller table for memory-constrained (e.g. embedded) targets,
/// e.g. `CompressorRefN::<512>::new()` for a 2 KB table. `N` must be a power of
/// two (checked at compile time).
///
/// This is the no-alloc API. With `alloc`, use [`Compressor`](crate::Compressor)
/// instead. For one-shot compression, use [`compress_into`] instead.
///
/// # Example
/// ```
/// use lz4rip_encode::{CompressorRef, get_maximum_output_size};
///
/// let mut comp = CompressorRef::new();
/// let input = b"hello world, hello world, hello!";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
pub struct CompressorRefN<const N: usize = DEFAULT_NODICT_ENTRIES> {
    table: HashTableU32<N>,
    stream_offset: usize,
}

/// A reusable no-dict block compressor with the standard 8 KB hash table.
pub type CompressorRef = CompressorRefN<DEFAULT_NODICT_ENTRIES>;

impl<const N: usize> fmt::Debug for CompressorRefN<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompressorRef").finish_non_exhaustive()
    }
}

impl<const N: usize> CompressorRefN<N> {
    /// Create a new compressor without a dictionary.
    #[must_use]
    pub fn new() -> Self {
        CompressorRefN {
            table: HashTableU32::<N>::new(),
            stream_offset: 0,
        }
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`]`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        compress_plain_table(&mut self.table, &mut self.stream_offset, input, output)
    }

    /// Compress `input` into a new `Vec<u8>`.
    #[cfg(feature = "alloc")]
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        let max_compressed = get_maximum_output_size(input.len());
        let mut compressed = vec![0u8; max_compressed];
        let compressed_len = self.compress_into(input, &mut compressed).unwrap();
        compressed.truncate(compressed_len);

        compressed
    }
}

/// A reusable dict block compressor (borrowing) with `N` entries per table.
///
/// [`DictCompressorRef`] is the standard-sized alias (two 8 KB tables). Use this
/// generic form to pick smaller tables for memory-constrained targets, e.g.
/// `DictCompressorRefN::<1024>::new(dict)` for two 2 KB tables. `N` must be a
/// power of two (checked at compile time).
///
/// This is the no-alloc dict API. With `alloc`, use
/// [`DictCompressor`](crate::DictCompressor) instead. Without a dictionary, use
/// [`CompressorRef`].
///
/// # Example
/// ```
/// use lz4rip_encode::{DictCompressorRef, get_maximum_output_size};
///
/// let dict = b"the quick brown fox";
/// let mut comp = DictCompressorRef::new(dict);
/// let input = b"the quick brown fox jumps";
/// let mut output = vec![0u8; get_maximum_output_size(input.len())];
/// let compressed_len = comp.compress_into(input, &mut output).unwrap();
/// ```
pub struct DictCompressorRefN<'a, const N: usize = DEFAULT_DICT_ENTRIES> {
    table: HashTableU32U16<N>,
    pristine: HashTableU32U16<N>,
    dict: &'a [u8],
}

/// A reusable dict block compressor (borrowing) with the standard 8 KB tables.
pub type DictCompressorRef<'a> = DictCompressorRefN<'a, DEFAULT_DICT_ENTRIES>;

impl<const N: usize> fmt::Debug for DictCompressorRefN<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DictCompressorRef")
            .field("dict_len", &self.dict.len())
            .finish()
    }
}

impl<'a, const N: usize> DictCompressorRefN<'a, N> {
    /// Create a new compressor seeded with an external dictionary.
    ///
    /// If `dict` is longer than the LZ4 window it is trimmed to the last
    /// [`WINDOW_SIZE`](lz4rip_core::WINDOW_SIZE) bytes. A dictionary shorter than
    /// 4 bytes is ignored (no dict matches); use [`CompressorRef`] for that case.
    #[must_use]
    pub fn new(dict: &'a [u8]) -> Self {
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
        DictCompressorRefN {
            table: HashTableU32U16::<N>::new(),
            pristine,
            dict: trimmed,
        }
    }

    /// Compress `input` into `output`, returning the number of compressed bytes.
    ///
    /// `output` must be at least [`get_maximum_output_size`]`(input.len())` bytes.
    pub fn compress_into(
        &mut self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, CompressError> {
        compress_dict_tables(
            &mut self.table,
            &mut self.pristine,
            self.dict,
            input,
            output,
        )
    }

    /// Compress `input` into a new `Vec<u8>`.
    #[cfg(feature = "alloc")]
    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        let max_compressed = get_maximum_output_size(input.len());
        let mut compressed = vec![0u8; max_compressed];
        let compressed_len = self.compress_into(input, &mut compressed).unwrap();
        compressed.truncate(compressed_len);

        compressed
    }
}

/// Compress `input` using the dict main + pristine tables. Shared by
/// [`CompressorRef::compress_into`] and the owning [`Compressor`] so the dict
/// branch logic lives in one place regardless of how the tables are stored.
pub(crate) fn compress_dict_tables<const N: usize>(
    table: &mut HashTableU32U16<N>,
    pristine: &mut HashTableU32U16<N>,
    dict: &[u8],
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, CompressError> {
    if input.len() <= DICT_READONLY_LIMIT && dict.len() + input.len() < u16::MAX as usize {
        compress_internal::<_, true, true, true, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            pristine,
            dict,
            dict.len(),
        )
    } else if dict.len() + input.len() < u16::MAX as usize {
        table.clear();
        compress_with_dict_table(
            input,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            pristine,
            dict,
            dict.len(),
        )
    } else {
        // dict + input >= 64 KB: positions overflow u16, so use a u32 table sized
        // to this compressor's `N` (honors the const-generic knob instead of
        // allocating a standard 8 KB table).
        let mut u32_table = HashTableU32::<N>::new();
        let mut dict_data = dict;
        init_dict(&mut u32_table, &mut dict_data);
        compress_internal::<_, true, true, false, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            &mut u32_table,
            dict_data,
            dict_data.len(),
        )
    }
}

/// Compress `input` using the plain (no-dict) table with epoch-based reuse.
/// Shared by [`CompressorRef::compress_into`] and the owning [`Compressor`].
pub(crate) fn compress_plain_table<const N: usize>(
    table: &mut HashTableU32<N>,
    stream_offset: &mut usize,
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, CompressError> {
    let offset = prepare_plain_table(table, stream_offset, input.len());
    if offset > 0 {
        compress_internal::<_, false, true, false, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            b"",
            offset,
        )
    } else {
        compress_internal::<_, false, false, false, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            b"",
            0,
        )
    }
}

#[inline]
fn prepare_plain_table<const N: usize>(
    table: &mut HashTableU32<N>,
    stream_offset: &mut usize,
    input_len: usize,
) -> usize {
    if input_len > EPOCH_THRESHOLD {
        table.clear();
        *stream_offset = input_len + MAX_DISTANCE + 1;
        return 0;
    }
    let offset = *stream_offset;
    let next = offset
        .checked_add(input_len)
        .and_then(|v| v.checked_add(MAX_DISTANCE + 1));
    if let Some(next) = next.filter(|&n| n <= u32::MAX as usize) {
        *stream_offset = next;
    } else {
        table.clear();
        *stream_offset = input_len + MAX_DISTANCE + 1;
    }
    offset
}

impl<const N: usize> Default for CompressorRefN<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_same_bytes(input: &[u8], cur: &mut usize, source: &[u8], candidate: usize) -> usize {
        const USIZE_SIZE: usize = core::mem::size_of::<usize>();
        let cur_slice = &input[*cur..input.len() - END_OFFSET];
        let cand_slice = &source[candidate..];

        let mut num = 0;
        for (block1, block2) in cur_slice
            .chunks_exact(USIZE_SIZE)
            .zip(cand_slice.chunks_exact(USIZE_SIZE))
        {
            let input_block = usize::from_ne_bytes(block1.try_into().unwrap());
            let match_block = usize::from_ne_bytes(block2.try_into().unwrap());

            if input_block == match_block {
                num += USIZE_SIZE;
            } else {
                let diff = input_block ^ match_block;
                num += (diff.to_le().trailing_zeros() / 8) as usize;
                *cur += num;
                return num;
            }
        }

        #[cold]
        fn count_same_bytes_tail(a: &[u8], b: &[u8], offset: usize) -> usize {
            a.iter()
                .zip(b)
                .skip(offset)
                .take_while(|(a, b)| a == b)
                .count()
        }
        num += count_same_bytes_tail(cur_slice, cand_slice, num);

        *cur += num;
        num
    }

    #[test]
    fn test_count_same_bytes() {
        let first: &[u8] = &[
            1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let second: &[u8] = &[
            1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        ];
        assert_eq!(count_same_bytes(first, &mut 0, second, 0), 16);

        for diff_idx in 8..100 {
            let first: Vec<u8> = (0u8..255).cycle().take(100 + 12).collect();
            let mut second = first.clone();
            second[diff_idx] = 255;
            for start in 0..=diff_idx {
                let same_bytes = count_same_bytes(&first, &mut start.clone(), &second, start);
                assert_eq!(same_bytes, diff_idx - start);
            }
        }
    }

    #[test]
    fn test_bug() {
        let input: &[u8] = &[
            10, 12, 14, 16, 18, 10, 12, 14, 16, 18, 10, 12, 14, 16, 18, 10, 12, 14, 16, 18,
        ];
        let mut output = [0u8; get_maximum_output_size(20)];
        let _ = compress_into(input, &mut output).unwrap();
    }

    #[test]
    fn test_conformant_last_block() {
        let aaas: &[u8] = b"aaaaaaaaaaaaaaa";

        let mut out = [0u8; get_maximum_output_size(15)];
        let n = compress_into(&aaas[..12], &mut out).unwrap();
        assert!(n > 12);
        let n = compress_into(&aaas[..13], &mut out).unwrap();
        assert!(n <= 13);
        let n = compress_into(&aaas[..14], &mut out).unwrap();
        assert!(n <= 14);
        let n = compress_into(&aaas[..15], &mut out).unwrap();
        assert!(n <= 15);
    }
}
