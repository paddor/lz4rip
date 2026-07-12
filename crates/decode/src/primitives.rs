//! Low-level memory primitives for decompression.

/// Read 1 byte (bounds-checked).
#[inline]
pub(crate) fn read_byte_inbounds(input: &[u8], n: usize) -> u8 {
    input[n]
}

/// Read 2 bytes as little-endian u16 (bounds-checked).
#[inline]
pub(crate) fn read_u16_inbounds(input: &[u8], n: usize) -> u16 {
    u16::from_le_bytes(input[n..n + 2].try_into().unwrap())
}

/// Copy 16 bytes from `src` to `dst`, advancing `dst_pos` by `advance`.
#[inline]
pub(crate) fn wild_copy_16(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    advance: usize,
) {
    debug_assert!(advance <= 16);
    dst[*dst_pos..*dst_pos + 16].copy_from_slice(&src[src_pos..src_pos + 16]);
    *dst_pos += advance;
}

/// Fixed 18-byte match copy within `buf`.
///
/// When the match does not overlap its own output (`offset >= advance`), a
/// single fixed-size `copy_within` of 18 bytes lets the compiler emit one
/// vectorized copy (the 18 - advance trailing bytes are harmless slack, covered
/// by the caller's headroom). The rare overlapping case (`offset < advance`,
/// period `offset >= 8`) falls back to fixed 8/8/2 chunks so each chunk reads
/// a non-overlapping region and the pattern repeats.
#[inline]
pub(crate) fn wild_match_copy_18(
    buf: &mut [u8],
    src_pos: usize,
    dst_pos: &mut usize,
    advance: usize,
) {
    debug_assert!(*dst_pos - src_pos >= 8);
    debug_assert!(advance <= 18);
    let dst = *dst_pos;
    if dst - src_pos >= advance {
        buf.copy_within(src_pos..src_pos + 18, dst);
    } else {
        buf.copy_within(src_pos..src_pos + 8, dst);
        buf.copy_within(src_pos + 8..src_pos + 16, dst + 8);
        buf.copy_within(src_pos + 16..src_pos + 18, dst + 16);
    }
    *dst_pos += advance;
}

/// Copy `len` bytes from `src` to `dst` (bounds-checked).
#[inline]
pub(crate) fn copy_from_src(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    len: usize,
) {
    dst[*dst_pos..*dst_pos + len].copy_from_slice(&src[src_pos..src_pos + len]);
    *dst_pos += len;
}

/// Overlapping match copy for offset >= 2.
///
/// Seeds `offset` bytes, then doubles the written region each step. Every
/// `copy_within` here is between non-overlapping ranges (source ends exactly
/// where destination begins), so it lowers to a plain `memcpy`. O(log) copies
/// instead of O(match_len), which matters for long repetitive matches.
#[inline]
pub(crate) fn copy_within_overlapping(
    buf: &mut [u8],
    start: usize,
    dst_pos: &mut usize,
    match_len: usize,
    offset: usize,
) {
    debug_assert!(offset >= 2);
    debug_assert!(*dst_pos == start + offset);
    let dst = *dst_pos;
    let initial = offset.min(match_len);
    buf.copy_within(start..start + initial, dst);
    let mut written = initial;
    while written < match_len {
        let copy_len = written.min(match_len - written);
        buf.copy_within(dst..dst + copy_len, dst + written);
        written += copy_len;
    }
    *dst_pos += match_len;
}

/// Literal copy of exactly `len` bytes from `src` to `dst`.
#[inline]
pub(crate) fn wild_copy_literals(
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

/// Match wildcopy for `offset >= 8`.
#[inline]
pub(crate) fn wild_copy_match_8(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 8);
    match_copy(buf, src, dst_pos, len);
}

/// Match wildcopy for `offset >= 16`.
#[inline]
pub(crate) fn wild_copy_match_16(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 16);
    match_copy(buf, src, dst_pos, len);
}

/// Match wildcopy for `offset >= 32`.
#[inline]
pub(crate) fn wild_copy_match_32(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 32);
    match_copy(buf, src, dst_pos, len);
}

/// Copy `len` bytes within `buf` from `src` to `*dst_pos`.
///
/// Non-overlapping (`offset >= len`): one `copy_within` of exactly `len` bytes,
/// a plain `memcpy`. Overlapping (`offset < len`): doubling via
/// [`copy_within_overlapping`], O(log) copies.
#[inline]
fn match_copy(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    let dst = *dst_pos;
    let offset = dst - src;
    if offset >= len {
        buf.copy_within(src..src + len, dst);
        *dst_pos += len;
    } else {
        copy_within_overlapping(buf, src, dst_pos, len, offset);
    }
}

/// Non-overlapping copy within `buf` (bounds-checked).
#[inline]
pub(crate) fn copy_within_nonoverlap(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(src + len <= *dst_pos);
    buf.copy_within(src..src + len, *dst_pos);
    *dst_pos += len;
}
