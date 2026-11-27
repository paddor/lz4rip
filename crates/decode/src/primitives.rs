//! Low-level memory primitives for decompression.
//!
//! All primitives are safe. Copies move fixed-size chunks (`first_chunk`), so
//! each chunk costs one length check instead of one per byte, and the
//! compiler emits plain vector loads and stores. Wild copies may write up to
//! one chunk past the requested length; callers reserve that slack. A chunk
//! is loaded completely before it is stored, so a chunk whose source overlaps
//! its destination repeats the pattern like the byte-by-byte copy would,
//! provided the offset is at least the chunk size.

use crate::decompress::SHORT_MATCH_MAX;

/// Chunk size of short match copies. A chunked copy is correct for offsets of
/// at least its chunk size. Shorter offsets, from `MIN_COPY_OFFSET`, use
/// the pattern copy, which also stores `MATCH_CHUNK` bytes at a time.
pub(crate) const MATCH_CHUNK: usize = 8;
/// Chunk size of long match steps and short literal copies.
pub(crate) const WIDE_CHUNK: usize = 2 * MATCH_CHUNK;
/// Chunk size of long literal copies, and the slack long copies need.
pub(crate) const LONG_CHUNK: usize = 2 * WIDE_CHUNK;
/// Smallest offset copied rather than filled. Offset 1 is a fill.
pub(crate) const MIN_COPY_OFFSET: usize = 2;
/// Match length up to which `wild_copy_match_long` copies without a loop.
const LONG_UNROLLED: usize = 2 * LONG_CHUNK;

/// Clears the top bit of a position. A no-op for any valid slice index
/// (slices never exceed `isize::MAX` bytes), but it tells the compiler that
/// `i + k` cannot wrap. The decoder masks its input and output positions once
/// per token, so the chunk bounds checks below follow from its headroom
/// checks and are dropped.
#[inline(always)]
pub(crate) fn idx(i: usize) -> usize {
    i as u32 as usize
}

/// Borrow `N` bytes of `buf` at `at`.
#[inline(always)]
pub(crate) fn chunk<T, const N: usize>(buf: &[T], at: usize) -> &[T; N] {
    buf[at..at + N].try_into().unwrap()
}

/// Mutably borrow `N` bytes of `buf` at `at`.
#[inline(always)]
pub(crate) fn chunk_mut<T, const N: usize>(buf: &mut [T], at: usize) -> &mut [T; N] {
    (&mut buf[at..at + N]).try_into().unwrap()
}

/// The `len` bytes of `buf` that end at `end`.
///
/// Slices `..end` first, then cuts from the end. Callers have just tested
/// `end <= buf.len()` and `len <= end` in this shape (as `pos + k <= cap` and
/// `offset <= pos`), so the compiler drops both checks. Slicing
/// `end - len..end` directly would need it to prove the subtraction cannot
/// wrap, which it does not.
#[inline(always)]
fn tail_window<T>(buf: &mut [T], end: usize, len: usize) -> &mut [T] {
    let head = &mut buf[..end];
    let start = head.len() - len;
    &mut head[start..]
}

/// Load `N` output bytes at `at`.
#[inline(always)]
fn load<const N: usize>(buf: &[u8], at: usize) -> [u8; N] {
    *chunk(buf, at)
}

/// Store `N` output bytes at `at`.
#[inline(always)]
pub(crate) fn store<const N: usize>(buf: &mut [u8], at: usize, chunk: [u8; N]) {
    *chunk_mut(buf, at) = chunk;
}

/// Copy `N` bytes within `buf` from `src` to `dst`, loading before storing.
#[inline(always)]
fn copy_chunk<const N: usize>(buf: &mut [u8], src: usize, dst: usize) {
    let chunk = load::<N>(buf, src);
    store(buf, dst, chunk);
}

/// Copy match within `buf`: three sequential copies (`MATCH_CHUNK`,
/// `MATCH_CHUNK`, then the rest of `SHORT_MATCH_MAX`) from `*dst_pos - offset`
/// to `*dst_pos`. Correct for any offset >= `MATCH_CHUNK`. All copies index
/// one window, so only the window costs a bounds check.
#[inline(always)]
pub(crate) fn wild_match_copy_18(
    buf: &mut [u8],
    dst_pos: &mut usize,
    offset: usize,
    advance: usize,
) {
    const REST: usize = SHORT_MATCH_MAX - 2 * MATCH_CHUNK;
    debug_assert!(offset >= MATCH_CHUNK);
    debug_assert!(advance <= SHORT_MATCH_MAX);
    let dst = *dst_pos;
    let window = tail_window(buf, dst + SHORT_MATCH_MAX, offset + SHORT_MATCH_MAX);
    copy_chunk::<MATCH_CHUNK>(window, 0, offset);
    copy_chunk::<MATCH_CHUNK>(window, MATCH_CHUNK, offset + MATCH_CHUNK);
    copy_chunk::<REST>(window, 2 * MATCH_CHUNK, offset + 2 * MATCH_CHUNK);
    *dst_pos += advance;
}

/// Long match wildcopy for `offset >= WIDE_CHUNK`: sequential `WIDE_CHUNK`
/// steps, rounded up to `LONG_CHUNK`, from `dst - offset` to `dst`. Needs
/// `LONG_CHUNK` bytes of slack past `dst + len`. All copies index one window.
/// The first `LONG_UNROLLED` bytes need no further bounds checks; only longer
/// matches loop with checks. A longer match that does not overlap its source
/// is one exact copy instead.
#[inline(always)]
pub(crate) fn wild_copy_match_long(buf: &mut [u8], dst: usize, offset: usize, len: usize) {
    debug_assert!(offset >= WIDE_CHUNK);
    if len > LONG_UNROLLED && offset >= len {
        // Long and disjoint from its source: one exact copy.
        let (history, out) = buf.split_at_mut(dst);
        out[..len].copy_from_slice(&history[dst - offset..][..len]);
        return;
    }
    let window = tail_window(buf, dst + len + LONG_CHUNK, offset + len + LONG_CHUNK);
    copy_chunk::<WIDE_CHUNK>(window, 0, offset);
    copy_chunk::<WIDE_CHUNK>(window, WIDE_CHUNK, offset + WIDE_CHUNK);
    if len > LONG_CHUNK {
        copy_chunk::<WIDE_CHUNK>(window, LONG_CHUNK, offset + LONG_CHUNK);
        copy_chunk::<WIDE_CHUNK>(
            window,
            LONG_CHUNK + WIDE_CHUNK,
            offset + LONG_CHUNK + WIDE_CHUNK,
        );
        let mut done = LONG_UNROLLED;
        while done < len {
            copy_chunk::<WIDE_CHUNK>(window, done, offset + done);
            copy_chunk::<WIDE_CHUNK>(window, done + WIDE_CHUNK, offset + done + WIDE_CHUNK);
            done += LONG_CHUNK;
        }
    }
}

/// Match wildcopy for `MIN_COPY_OFFSET <= offset < MATCH_CHUNK`. Spreads
/// the `offset`-byte period over `MATCH_CHUNK` bytes, then stores them every
/// `MATCH_CHUNK - MATCH_CHUNK % offset` bytes, a multiple of the period, so
/// each store continues the pattern. Needs `MATCH_CHUNK` bytes of slack past
/// `*dst_pos + len`.
#[inline(always)]
pub(crate) fn wild_copy_match_short_offset(
    buf: &mut [u8],
    dst_pos: &mut usize,
    offset: usize,
    len: usize,
) {
    debug_assert!((MIN_COPY_OFFSET..MATCH_CHUNK).contains(&offset));
    let dst = *dst_pos;
    let window = tail_window(buf, dst + len + MATCH_CHUNK, offset + len + MATCH_CHUNK);
    let mut pattern = [window[0]; MATCH_CHUNK];
    // Bytes below `MIN_COPY_OFFSET` are always history. Copying them first
    // shortens the loop, which keeps Kani's end-to-end unwind bound low.
    pattern[..MIN_COPY_OFFSET].copy_from_slice(&window[..MIN_COPY_OFFSET]);
    for i in MIN_COPY_OFFSET..MATCH_CHUNK {
        pattern[i] = if i < offset {
            window[i]
        } else {
            pattern[i - offset]
        };
    }
    let step = MATCH_CHUNK - MATCH_CHUNK % offset;
    let mut done = 0;
    while done < len {
        *chunk_mut::<u8, MATCH_CHUNK>(window, offset + done) = pattern;
        done += step;
    }
    *dst_pos += len;
}

/// Copy exactly `len` bytes from `src[src_pos..]` to `dst[*dst_pos..]`.
#[inline(always)]
pub(crate) fn copy_from_src(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    len: usize,
) {
    let d = *dst_pos;
    dst[d..d + len].copy_from_slice(&src[src_pos..src_pos + len]);
    *dst_pos += len;
}

/// Overlapping match copy for offset >= `MIN_COPY_OFFSET`. Seeds with `offset` bytes, then
/// doubles the written region each iteration until `match_len` is reached.
/// Every copy is between disjoint ranges, so it lowers to `memcpy`.
/// O(log(match_len/offset)) copies.
#[inline(always)]
pub(crate) fn copy_within_overlapping(
    buf: &mut [u8],
    start: usize,
    dst_pos: &mut usize,
    match_len: usize,
    offset: usize,
) {
    debug_assert!(offset >= MIN_COPY_OFFSET);
    debug_assert!(*dst_pos == start + offset);
    let dst = *dst_pos;
    let initial = offset.min(match_len);
    let (history, out) = buf.split_at_mut(dst);
    let out = &mut out[..match_len];
    out[..initial].copy_from_slice(&history[start..start + initial]);
    let mut written = initial;
    while written < match_len {
        let copy_len = written.min(match_len - written);
        let (done, rest) = out.split_at_mut(written);
        rest[..copy_len].copy_from_slice(&done[..copy_len]);
        written += copy_len;
    }
    *dst_pos += match_len;
}

/// Copy `len` literal bytes from `src[src_pos..]` to `dst[dst_pos..]` in
/// `LONG_CHUNK` chunks. Both need `LONG_CHUNK` bytes of slack past `len`. Two
/// window checks, then no per-chunk checks.
#[inline(always)]
pub(crate) fn copy_literals_32(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: usize,
    len: usize,
) {
    let n = len.div_ceil(LONG_CHUNK) * LONG_CHUNK;
    let s = &src[src_pos..][..n];
    let d = &mut dst[dst_pos..][..n];
    for (d, s) in d
        .as_chunks_mut::<LONG_CHUNK>()
        .0
        .iter_mut()
        .zip(s.as_chunks::<LONG_CHUNK>().0)
    {
        *d = *s;
    }
}

/// Literal wildcopy in `LONG_CHUNK` chunks. Both `src` and `dst` need
/// `LONG_CHUNK` bytes of slack past `len`.
#[inline(always)]
pub(crate) fn wild_copy_literals(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    len: usize,
) {
    copy_literals_32(src, src_pos, dst, *dst_pos, len);
    *dst_pos += len;
}

/// Match wildcopy for `offset >= STEP` in `STEP`-byte chunks, from
/// `*dst_pos - offset` to `*dst_pos`. Needs `STEP` bytes of slack past
/// `*dst_pos + len`. All copies index one window, so the first chunk needs no
/// bounds check of its own.
#[inline(always)]
fn wild_copy_match<const STEP: usize>(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    let offset = *dst_pos - src;
    debug_assert!(offset >= STEP);
    let window = tail_window(buf, *dst_pos + len + STEP, offset + len + STEP);
    let mut done = 0;
    loop {
        copy_chunk::<STEP>(window, done, offset + done);
        done += STEP;
        if done >= len {
            break;
        }
    }
    *dst_pos += len;
}

/// Match wildcopy for `offset >= MATCH_CHUNK`, one `MATCH_CHUNK` per step.
/// Needs `MATCH_CHUNK` bytes of slack.
#[inline(always)]
pub(crate) fn wild_copy_match_8(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    wild_copy_match::<MATCH_CHUNK>(buf, src, dst_pos, len);
}

/// Match wildcopy for `offset >= WIDE_CHUNK`, one `WIDE_CHUNK` per step. Needs
/// `WIDE_CHUNK` bytes of slack.
#[inline(always)]
pub(crate) fn wild_copy_match_16(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    wild_copy_match::<WIDE_CHUNK>(buf, src, dst_pos, len);
}

/// Match wildcopy for `offset >= WIDE_CHUNK`, one `LONG_CHUNK` per step. Needs
/// `LONG_CHUNK` bytes of slack.
#[inline(always)]
pub(crate) fn wild_copy_match_32(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    let dst = *dst_pos;
    wild_copy_match_long(buf, dst, dst - src, len);
    *dst_pos += len;
}

/// Non-overlapping copy of `len` bytes within `buf` (`src + len <= *dst_pos`).
#[inline(always)]
pub(crate) fn copy_within_nonoverlap(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(src + len <= *dst_pos);
    let (history, out) = buf.split_at_mut(*dst_pos);
    out[..len].copy_from_slice(&history[src..src + len]);
    *dst_pos += len;
}
