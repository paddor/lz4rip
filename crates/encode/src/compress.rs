//! LZ4 block compression.

use core::fmt;

use crate::hashtable::HashTable;
use crate::hashtable::{count_same_bytes, first4, rd32, rd64, rd64_upto};
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
use lz4rip_core::slice_copy;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

pub(crate) use crate::hashtable::HashTableU32;
pub(crate) use crate::hashtable::HashTableU32U16;
pub use crate::hashtable::{DEFAULT_DICT_ENTRIES, DEFAULT_NODICT_ENTRIES, MIN_ENTRIES};

/// Inputs up to this size reuse the no-dict hash table across calls (epoch-based
/// table reuse); larger inputs clear it. Independent of table entry count.
///
/// Reuse saves the table clear, which dominates tiny inputs. From 2 KB up a
/// cleared table is faster on compressible data: a table full of stale
/// entries costs more than the clear (see DESIGN.md).
const EPOCH_THRESHOLD: usize = 1024;

/// Skip acceleration: step grows by 1 every `1 << N` consecutive non-matches.
/// C lz4 uses 6; see DESIGN.md for tradeoff analysis.
/// Keep this constant even for small blocks: denser search costs substantially
/// more CPU on rotating messages, especially at 1-2 KiB.
const INCREASE_STEPSIZE_BITSHIFT: usize = 3;

/// Inputs up to this size use the dict table read-only (no per-call clearing or
/// table writes). Self-references within small inputs are rare; the dict provides
/// virtually all matches. Skips the 8 KB table.clear() and all put_at writes.
const DICT_READONLY_LIMIT: usize = 256;

/// Dict inputs up to this size reuse the main table across calls (epoch
/// reuse) instead of clearing it. Above it, reuse gained nothing on a stream
/// of different messages and made repeated 8 KB text 20-30% slower.
const DICT_EPOCH_THRESHOLD: usize = 1024;

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

// The compress loops write into `rest`, the unwritten tail of the output
// buffer. Each write splits bytes off its front: one length test per write,
// and the window takes two registers (pointer, length) where a buffer and
// position take three.

/// Write `byte` to the front of `rest`.
#[inline(always)]
fn put_byte(rest: &mut [u8], byte: u8) -> &mut [u8] {
    let (first, tail) = rest.split_first_mut().unwrap();
    *first = byte;
    tail
}

/// Write `bytes` to the front of `rest`.
#[inline(always)]
fn put_slice<'a>(rest: &'a mut [u8], bytes: &[u8]) -> &'a mut [u8] {
    let (dst, tail) = rest.split_at_mut(bytes.len());
    slice_copy(bytes, dst);
    tail
}

/// Write a variable-length integer in the LZ4 encoding to the front of `rest`.
#[inline(always)]
fn put_integer(mut rest: &mut [u8], mut n: usize) -> &mut [u8] {
    while n >= 0xFF {
        n -= 0xFF;
        rest = put_byte(rest, 0xFF);
    }
    put_byte(rest, n as u8)
}

/// Write the final literal run `input[start..]` and return the bytes left
/// in `rest`.
#[cold]
fn handle_last_literals(rest: &mut [u8], input: &[u8], start: usize) -> usize {
    let lit_len = input.len() - start;
    let mut rest = put_byte(rest, token_from_literal(lit_len));
    if lit_len >= 0xF {
        rest = put_integer(rest, lit_len - 0xF);
    }
    put_slice(rest, &input[start..]).len()
}

/// Mark the bytes up to `rest_len` written: `output` held `total` bytes
/// after `start`. Returns the number written.
#[inline(always)]
fn finish_output(pos: &mut usize, start: usize, total: usize, rest_len: usize) -> usize {
    let written = total - rest_len;
    *pos = start + written;
    written
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
///
/// `NEAR`: dict and input fit in one window, so every candidate from this
/// stream is within `MAX_DISTANCE` and needs no distance test.
#[inline(never)]
pub(crate) fn compress_internal<
    T: HashTable,
    const USE_DICT: bool,
    const HAS_OFFSET: bool,
    const READONLY: bool,
    const NEAR: bool,
    S: Sink,
>(
    input: &[u8],
    input_pos: usize,
    output: &mut S,
    table: &mut T,
    ext_dict: &[u8],
    input_stream_offset: usize,
) -> Result<usize, CompressError> {
    let input = if NEAR {
        input
    } else {
        core::hint::black_box(input)
    };
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
    if READONLY {
        debug_assert!(USE_DICT && input_stream_offset == ext_dict.len());
        debug_assert!(ext_dict.len() + input.len() < u16::MAX as usize);
    }
    if NEAR {
        assert!(ext_dict.len() + input.len() <= MAX_DISTANCE + 1);
    }
    let input_stream_offset = if HAS_OFFSET { input_stream_offset } else { 0 };
    let (out, out_pos) = output.output_mut_with_pos();
    let out_start = *out_pos;
    let mut rest = &mut out[out_start..];
    let out_total = rest.len();
    if out_total < get_maximum_output_size(input.len() - input_pos) {
        return Err(CompressError::OutputTooSmall);
    }

    if input.len() - input_pos < LZ4_MIN_LENGTH {
        let left = handle_last_literals(rest, input, input_pos);
        return Ok(finish_output(out_pos, out_start, out_total, left));
    }

    let ext_dict_stream_offset = input_stream_offset - ext_dict.len();
    let end_pos_check = input.len() - MFLIMIT;
    let mut literal_start = input_pos;
    let mut cur = input_pos;

    if cur == 0 && input_stream_offset == 0 {
        let hash = T::get_hash_at(input, 0);
        if !READONLY {
            table.put_at(hash, 0);
        }
        cur = 1;
    }

    // Eight bytes at the next search position and their hash.
    let mut forward_bytes = rd64(input, cur);
    let mut forward_hash = T::hash(forward_bytes);

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
                let left = handle_last_literals(rest, input, literal_start);
                return Ok(finish_output(out_pos, out_start, out_total, left));
            }

            let hash = forward_hash;
            let curr_bytes = if NEAR {
                // `cur` passed the exit test as the previous `next_cur` (or,
                // after a match, the `cur <= end_pos_check` test). The assert
                // compares against the limit already in a register and
                // covers the read; reusing `forward_bytes` would take a
                // register.
                assert!(cur <= end_pos_check + 1);
                rd32(input, cur)
            } else {
                // Inputs over 64 KiB: reuse the bytes read for hashing.
                first4(forward_bytes)
            };
            candidate = table.get_at(hash);
            forward_bytes = rd64(input, next_cur);
            forward_hash = T::hash(forward_bytes);
            if !READONLY {
                table.put_at(hash, cur + input_stream_offset);
            }

            debug_assert!(READONLY || candidate <= input_stream_offset + cur);

            // Each branch reads the candidate's 4 bytes under a test the
            // compiler can see bounds the read, so it drops the check.
            let cand_bytes;
            let input_pos = candidate.wrapping_sub(input_stream_offset);
            if READONLY {
                // Only the pristine dict table: every entry is a dict
                // position, and dict + input fit in `u16`, so it is within
                // `MAX_DISTANCE`. `init_dict` stores only positions with 8
                // readable bytes; the bound check covers an empty dict.
                if candidate + 4 > ext_dict.len() {
                    cur = next_cur;
                    continue;
                }
                debug_assert!(input_stream_offset + cur - candidate <= MAX_DISTANCE);
                // The dict starts the stream: `ext_dict_stream_offset` is 0.
                debug_assert_eq!(ext_dict_stream_offset, 0);
                offset = (input_stream_offset + cur - candidate) as u16;
                candidate_source = ext_dict;
                cand_bytes = rd32(ext_dict, candidate);
            } else if input_pos < next_cur
                && (!(HAS_OFFSET && !USE_DICT) || cur - input_pos <= MAX_DISTANCE)
            {
                // Table entries from this stream sit below
                // `input_stream_offset + cur`, so `input_pos < cur`;
                // `next_cur` passed the exit test. Most candidates fail the
                // byte compare, so the distance test comes after it. The
                // epoch path keeps an always-true distance test up front:
                // without it, it measured 5-10% slower at 512 B (branch
                // misses on the staleness test).
                if rd32(input, input_pos) == curr_bytes && (NEAR || cur - input_pos <= MAX_DISTANCE)
                {
                    offset = (cur - input_pos) as u16;
                    candidate = input_pos;
                    candidate_source = input;
                    break;
                }
                cur = next_cur;
                continue;
            } else if USE_DICT
                && candidate >= ext_dict_stream_offset
                && candidate - ext_dict_stream_offset + 4 <= ext_dict.len()
            {
                // `candidate` sits below `input_stream_offset` here. Distance
                // test after the byte compare, as above.
                let dict_pos = candidate - ext_dict_stream_offset;
                if rd32(ext_dict, dict_pos) == curr_bytes
                    && (NEAR || input_stream_offset + cur - candidate <= MAX_DISTANCE)
                {
                    offset = (input_stream_offset + cur - candidate) as u16;
                    candidate = dict_pos;
                    candidate_source = ext_dict;
                    break;
                }
                cur = next_cur;
                continue;
            } else {
                cur = next_cur;
                continue;
            }
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
                count_same_bytes(input, &mut cur, candidate_source, candidate, END_OFFSET);

            let hash = T::hash(rd64_upto(input, cur - 2, input.len() - 8));
            if !READONLY {
                table.put_at(hash, cur - 2 + input_stream_offset);
            }

            rest = emit_sequence(
                rest,
                input,
                literal_start,
                lit_len,
                offset,
                duplicate_length,
            );
            literal_start = cur;

            if !USE_DICT && cur <= end_pos_check {
                let bytes = rd64(input, cur);
                let hash = T::hash(bytes);
                let rematch = table.get_at(hash);

                let rc = rematch.wrapping_sub(input_stream_offset);
                if rc < cur
                    && first4(bytes) == rd32(input, rc)
                    && (NEAR || cur - rc <= MAX_DISTANCE)
                {
                    table.put_at(hash, cur + input_stream_offset);
                    candidate = rc;
                    candidate_source = input;
                    offset = (cur - rc) as u16;
                    continue;
                }
                forward_bytes = bytes;
                forward_hash = hash;
            } else if cur <= end_pos_check {
                forward_bytes = rd64(input, cur);
                forward_hash = T::hash(forward_bytes);
            }
            break;
        }
    }
}

/// Compress with a caller-owned `HashTableU32`.
///
/// This is cross-crate plumbing for the frame encoder. It keeps the internal
/// `HashTable` trait private.
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
    compress_internal::<_, USE_DICT, HAS_OFFSET, READONLY, false, _>(
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
    while i + 8 <= input.len() {
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
    main_base: usize,
) -> Result<usize, CompressError> {
    debug_assert_eq!(input_stream_offset, ext_dict.len());
    debug_assert!(main_base + input.len() <= u16::MAX as usize);
    assert!(ext_dict.len() <= WINDOW_SIZE);
    #[cfg(target_pointer_width = "32")]
    ensure_stream_bounds(input_stream_offset, input.len(), ext_dict.len())?;
    #[cfg(not(target_pointer_width = "32"))]
    assert_stream_bounds(input_stream_offset, input.len(), ext_dict.len());
    let (out, out_pos) = output.output_mut_with_pos();
    let out_start = *out_pos;
    let mut rest = &mut out[out_start..];
    let out_total = rest.len();
    if out_total < get_maximum_output_size(input.len()) {
        return Err(CompressError::OutputTooSmall);
    }

    if input.len() < LZ4_MIN_LENGTH {
        let left = handle_last_literals(rest, input, 0);
        return Ok(finish_output(out_pos, out_start, out_total, left));
    }

    let end_pos_check = input.len() - MFLIMIT;
    let mut literal_start = 0;

    let hash = T::get_hash_at(input, 0);
    table.put_at(hash, main_base);
    let mut cur = 1;

    let mut forward_hash = T::get_hash_at(input, cur);

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
                let left = handle_last_literals(rest, input, literal_start);
                return Ok(finish_output(out_pos, out_start, out_total, left));
            }

            let hash = forward_hash;
            // See `compress_internal`.
            assert!(cur <= end_pos_check + 1);
            let curr_bytes = rd32(input, cur);
            forward_hash = T::get_hash_at(input, next_cur);

            let main_candidate = table.get_at(hash);
            table.put_at(hash, cur + main_base);

            // Probe dict table first: for small inputs most matches come from
            // the dict, and even for large inputs the dict covers the first
            // window of repeated structure.
            let dict_candidate = dict_table.get_at(hash);
            // Pristine entries are dict positions, and dict + input fit in
            // `u16`, so a dict candidate is always within `MAX_DISTANCE`.
            // `init_dict` stores only positions with 8 readable bytes, so
            // requiring 4 loses nothing and lets the compiler drop the read's
            // bounds check (`dict_candidate` is a `u16`, so the add can't
            // wrap).
            if dict_candidate + 4 <= ext_dict.len() {
                debug_assert!(input_stream_offset + cur - dict_candidate <= MAX_DISTANCE);
                if rd32(ext_dict, dict_candidate) == curr_bytes {
                    offset = (input_stream_offset + cur - dict_candidate) as u16;
                    candidate = dict_candidate;
                    candidate_source = ext_dict;
                    break;
                }
            }

            // Main-table entries from earlier calls sit below `main_base`,
            // entries from this call below `main_base + cur`. Dict + input fit in `u16`, so every
            // candidate is within `MAX_DISTANCE`. Comparing against
            // `next_cur`, which passed the exit test, lets the compiler drop
            // the read's bounds check.
            // The first test is redundant (`main_pos` wraps), but dropping it
            // measured 2-3% slower: more branch misses.
            let main_pos = main_candidate.wrapping_sub(main_base);
            if main_candidate >= main_base && main_pos < next_cur {
                debug_assert!(main_pos < cur);
                if rd32(input, main_pos) == curr_bytes {
                    offset = (cur - main_pos) as u16;
                    candidate = main_pos;
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
        let duplicate_length =
            count_same_bytes(input, &mut cur, candidate_source, candidate, END_OFFSET);

        let hash = T::hash(rd64_upto(input, cur - 2, input.len() - 8));
        table.put_at(hash, cur - 2 + main_base);

        rest = emit_sequence(
            rest,
            input,
            literal_start,
            lit_len,
            offset,
            duplicate_length,
        );
        literal_start = cur;

        if cur <= end_pos_check {
            forward_hash = T::get_hash_at(input, cur);
        }
    }
}

#[inline]
fn push_byte(output: &mut impl Sink, el: u8) {
    output.push(el);
}

/// Write one sequence (token, literal run, offset, match length) to the
/// front of `rest`.
#[inline(always)]
fn emit_sequence<'a>(
    rest: &'a mut [u8],
    input: &[u8],
    literal_start: usize,
    lit_len: usize,
    offset: u16,
    match_len: usize,
) -> &'a mut [u8] {
    if lit_len < 0xF && match_len < 0xF {
        // No length bytes. Copy 16 literal bytes regardless of the run
        // length (the offset overwrites the excess), so one length test
        // per side covers the sequence and no branch picks a copy size.
        let src = input.get(literal_start..literal_start + 16);
        if let (Some(seq), Some(src)) = (rest.first_chunk_mut::<19>(), src) {
            seq[0] = ((lit_len as u8) << 4) | match_len as u8;
            seq[1..17].copy_from_slice(src);
            seq[1 + lit_len..3 + lit_len].copy_from_slice(&offset.to_le_bytes());
            return &mut rest[3 + lit_len..];
        }
    }
    let literals = &input[literal_start..literal_start + lit_len];
    let mut rest = put_byte(
        rest,
        token_from_literal_and_match_length(lit_len, match_len),
    );
    if lit_len >= 0xF {
        rest = put_integer(rest, lit_len - 0xF);
    }
    rest = put_slice(rest, literals);
    rest = put_slice(rest, &offset.to_le_bytes());
    if match_len >= 0xF {
        rest = put_integer(rest, match_len - 0xF);
    }
    rest
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
        compress_internal::<_, USE_DICT, USE_DICT, false, true, _>(
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
        compress_internal::<_, USE_DICT, USE_DICT, false, false, _>(
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
    while i + 8 <= dict_data.len() {
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
    main_base: usize,
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
            main_base: 1,
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
            &mut self.main_base,
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
    main_base: &mut usize,
    dict: &[u8],
    input: &[u8],
    output: &mut [u8],
) -> Result<usize, CompressError> {
    if input.len() <= DICT_READONLY_LIMIT && dict.len() + input.len() < u16::MAX as usize {
        compress_internal::<_, true, true, true, true, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            pristine,
            dict,
            dict.len(),
        )
    } else if dict.len() + input.len() < u16::MAX as usize {
        // Main-table positions start at `main_base`, at least 1 so a cleared
        // table's zeros never pass as positions. Small inputs reuse the table
        // across calls (epoch reuse): earlier calls' entries sit below
        // `main_base`. Larger inputs clear it.
        if input.len() > DICT_EPOCH_THRESHOLD || *main_base + input.len() > u16::MAX as usize {
            table.clear();
            *main_base = 1;
        }
        let base = *main_base;
        *main_base += input.len();
        compress_with_dict_table(
            input,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            pristine,
            dict,
            dict.len(),
            base,
        )
    } else {
        // dict + input >= 64 KB: positions overflow u16, so use a u32 table sized
        // to this compressor's `N` (honors the const-generic knob instead of
        // allocating a standard 8 KB table).
        let mut u32_table = HashTableU32::<N>::new();
        let mut dict_data = dict;
        init_dict(&mut u32_table, &mut dict_data);
        compress_internal::<_, true, true, false, false, _>(
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
        // Epoch reuse: inputs up to `EPOCH_THRESHOLD`, and entries from
        // earlier calls fail the `input_pos < next_cur` test.
        compress_internal::<_, false, true, false, true, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            b"",
            offset,
        )
    } else if input.len() <= MAX_DISTANCE + 1 {
        compress_internal::<_, false, false, false, true, _>(
            input,
            0,
            &mut VerifiedSliceSink::new(output, 0),
            table,
            b"",
            0,
        )
    } else {
        compress_internal::<_, false, false, false, false, _>(
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
    let mut offset = *stream_offset;
    let next = offset
        .checked_add(input_len)
        .and_then(|v| v.checked_add(MAX_DISTANCE + 1));
    if let Some(next) = next.filter(|&n| n <= u32::MAX as usize) {
        *stream_offset = next;
    } else {
        table.clear();
        *stream_offset = input_len + MAX_DISTANCE + 1;
        offset = 0;
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
