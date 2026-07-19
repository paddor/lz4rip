//! Low-level memory primitives for decompression.
//!
//! Each operation has two implementations selected at compile time. The default
//! build uses unchecked indexing and unaligned reads for speed; these functions
//! are `unsafe fn` because callers must uphold bounds preconditions documented
//! in each function's `debug_assert!` guards. The `paranoid` feature provides
//! safe `fn` twins with no preconditions (violations panic via bounds checks).

/// Read 1 byte without bounds checking.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn read_byte_inbounds(input: &[u8], n: usize) -> u8 {
    debug_assert!(n < input.len());
    unsafe { *input.get_unchecked(n) }
}

/// Read 1 byte (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn read_byte_inbounds(input: &[u8], n: usize) -> u8 {
    input[n]
}

/// Read 2 bytes as little-endian u16 without bounds checking.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn read_u16_inbounds(input: &[u8], n: usize) -> u16 {
    debug_assert!(n + 2 <= input.len());
    unsafe {
        (input.as_ptr().add(n) as *const u16)
            .read_unaligned()
            .to_le()
    }
}

/// Read 2 bytes as little-endian u16 (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn read_u16_inbounds(input: &[u8], n: usize) -> u16 {
    u16::from_le_bytes(input[n..n + 2].try_into().unwrap())
}

/// Copy 16 bytes from `src[src_pos..]` to `dst[dst_pos..]`, advancing `dst_pos` by `advance`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_copy_16(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    advance: usize,
) {
    debug_assert!(src_pos + 16 <= src.len());
    debug_assert!(*dst_pos + 16 <= dst.len());
    debug_assert!(advance <= 16);
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr().add(src_pos),
            dst.as_mut_ptr().add(*dst_pos),
            16,
        );
    }
    *dst_pos += advance;
}

/// Copy 16 bytes from `src` to `dst` (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Copy match within `buf`: three sequential 8-byte copies from `src_pos` to `*dst_pos`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_match_copy_18(
    buf: &mut [u8],
    src_pos: usize,
    dst_pos: &mut usize,
    advance: usize,
) {
    debug_assert!(*dst_pos + 18 <= buf.len());
    debug_assert!(*dst_pos - src_pos >= 8);
    debug_assert!(advance <= 18);
    unsafe {
        let ptr = buf.as_mut_ptr();
        core::ptr::copy_nonoverlapping(ptr.add(src_pos), ptr.add(*dst_pos), 8);
        core::ptr::copy_nonoverlapping(ptr.add(src_pos + 8), ptr.add(*dst_pos + 8), 8);
        core::ptr::copy_nonoverlapping(ptr.add(src_pos + 16), ptr.add(*dst_pos + 16), 2);
    }
    *dst_pos += advance;
}

/// Fixed 18-byte match copy within `buf` (paranoid: bounds-checked).
///
/// Mirrors lz4_flex's safe fast path: when the match does not overlap its own
/// output (`offset >= advance`), a single fixed-size `copy_within` of 18 bytes
/// lets the compiler emit one vectorized copy (the 18 - advance trailing bytes
/// are harmless slack, covered by the caller's headroom). The rare overlapping
/// case (`offset < advance`, period `offset >= 8`) falls back to fixed 8/8/2
/// chunks so each chunk reads a non-overlapping region and the pattern repeats.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Unchecked copy from `src[src_pos..src_pos+len]` to `dst[*dst_pos..*dst_pos+len]`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn copy_from_src(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(src_pos + len <= src.len());
    debug_assert!(*dst_pos + len <= dst.len());
    unsafe {
        core::ptr::copy_nonoverlapping(
            src.as_ptr().add(src_pos),
            dst.as_mut_ptr().add(*dst_pos),
            len,
        );
    }
    *dst_pos += len;
}

/// Copy `len` bytes from `src` to `dst` (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Overlapping match copy for offset >= 2. Seeds with `offset` bytes via
/// non-overlapping copy, then doubles the written region each iteration
/// until `match_len` is reached. O(log(match_len/offset)) memcpy calls.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn copy_within_overlapping(
    buf: &mut [u8],
    start: usize,
    dst_pos: &mut usize,
    match_len: usize,
    offset: usize,
) {
    debug_assert!(offset >= 2);
    debug_assert!(*dst_pos == start + offset);
    debug_assert!(*dst_pos + match_len <= buf.len());

    let dst = *dst_pos;
    let initial = offset.min(match_len);

    unsafe {
        let ptr = buf.as_mut_ptr();
        core::ptr::copy_nonoverlapping(ptr.add(start), ptr.add(dst), initial);

        let mut written = initial;
        while written < match_len {
            let copy_len = written.min(match_len - written);
            core::ptr::copy_nonoverlapping(ptr.add(dst), ptr.add(dst + written), copy_len);
            written += copy_len;
        }
    }

    *dst_pos += match_len;
}

/// Overlapping match copy for offset >= 2 (paranoid: doubling, like the
/// unchecked version but with `copy_within`).
///
/// Seeds `offset` bytes, then doubles the written region each step. Every
/// `copy_within` here is between non-overlapping ranges (source ends exactly
/// where destination begins), so it lowers to a plain `memcpy`. O(log) copies
/// instead of O(match_len), which matters for long repetitive matches.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Inline literal wildcopy in 32-byte chunks.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_copy_literals(
    src: &[u8],
    src_pos: usize,
    dst: &mut [u8],
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(src_pos + len + 32 <= src.len());
    debug_assert!(*dst_pos + len + 32 <= dst.len());
    let d = *dst_pos;
    unsafe {
        let sp = src.as_ptr();
        let dp = dst.as_mut_ptr();
        let mut done = 0;
        loop {
            let v0 = (sp.add(src_pos + done) as *const u128).read_unaligned();
            let v1 = (sp.add(src_pos + done + 16) as *const u128).read_unaligned();
            (dp.add(d + done) as *mut u128).write_unaligned(v0);
            (dp.add(d + done + 16) as *mut u128).write_unaligned(v1);
            done += 32;
            if done >= len {
                break;
            }
        }
    }
    *dst_pos += len;
}

/// Literal copy of exactly `len` bytes from `src` to `dst` (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Inline match wildcopy for `offset >= 8`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_copy_match_8(
    buf: &mut [u8],
    src: usize,
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(*dst_pos >= src + 8);
    debug_assert!(*dst_pos + len + 8 <= buf.len());
    let dst = *dst_pos;
    unsafe {
        let ptr = buf.as_mut_ptr();
        let mut done = 0;
        loop {
            let v = (ptr.add(src + done) as *const u64).read_unaligned();
            (ptr.add(dst + done) as *mut u64).write_unaligned(v);
            done += 8;
            if done >= len {
                break;
            }
        }
    }
    *dst_pos += len;
}

/// Match wildcopy for `offset >= 8` (paranoid: 8-byte `copy_within` chunks).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn wild_copy_match_8(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 8);
    match_copy(buf, src, dst_pos, len);
}

/// Inline match wildcopy for `offset >= 16`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_copy_match_16(
    buf: &mut [u8],
    src: usize,
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(*dst_pos >= src + 16);
    debug_assert!(*dst_pos + len + 16 <= buf.len());
    let dst = *dst_pos;
    unsafe {
        let ptr = buf.as_mut_ptr();
        let mut done = 0;
        loop {
            let v = (ptr.add(src + done) as *const u128).read_unaligned();
            (ptr.add(dst + done) as *mut u128).write_unaligned(v);
            done += 16;
            if done >= len {
                break;
            }
        }
    }
    *dst_pos += len;
}

/// Match wildcopy for `offset >= 16` (paranoid: 16-byte `copy_within` chunks).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn wild_copy_match_16(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 16);
    match_copy(buf, src, dst_pos, len);
}

/// Inline match wildcopy for `offset >= 32`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn wild_copy_match_32(
    buf: &mut [u8],
    src: usize,
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(*dst_pos >= src + 32);
    debug_assert!(*dst_pos + len + 32 <= buf.len());
    let dst = *dst_pos;
    unsafe {
        let ptr = buf.as_mut_ptr();
        let mut done = 0;
        loop {
            let v0 = (ptr.add(src + done) as *const u128).read_unaligned();
            let v1 = (ptr.add(src + done + 16) as *const u128).read_unaligned();
            (ptr.add(dst + done) as *mut u128).write_unaligned(v0);
            (ptr.add(dst + done + 16) as *mut u128).write_unaligned(v1);
            done += 32;
            if done >= len {
                break;
            }
        }
    }
    *dst_pos += len;
}

/// Match wildcopy for `offset >= 32` (paranoid: 32-byte `copy_within` chunks).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn wild_copy_match_32(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(*dst_pos >= src + 32);
    match_copy(buf, src, dst_pos, len);
}

/// Copy `len` bytes within `buf` from `src` to `*dst_pos` (paranoid).
///
/// Non-overlapping (`offset >= len`): one `copy_within` of exactly `len` bytes,
/// a plain `memcpy`. Overlapping (`offset < len`): doubling via
/// [`copy_within_overlapping`], O(log) copies. Used by the `wild_copy_match_*`
/// safe twins; the unchecked versions split by offset for fixed-size SIMD
/// wildcopies, but in the safe build a single sized copy plus doubling is faster
/// and matches lz4_flex's strategy.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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

/// Unchecked non-overlapping copy within `buf`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline]
pub(crate) unsafe fn copy_within_nonoverlap(
    buf: &mut [u8],
    src: usize,
    dst_pos: &mut usize,
    len: usize,
) {
    debug_assert!(src + len <= *dst_pos);
    debug_assert!(*dst_pos + len <= buf.len());
    unsafe {
        let ptr = buf.as_mut_ptr();
        core::ptr::copy_nonoverlapping(ptr.add(src), ptr.add(*dst_pos), len);
    }
    *dst_pos += len;
}

/// Non-overlapping copy within `buf` (paranoid: bounds-checked).
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline]
pub(crate) fn copy_within_nonoverlap(buf: &mut [u8], src: usize, dst_pos: &mut usize, len: usize) {
    debug_assert!(src + len <= *dst_pos);
    buf.copy_within(src..src + len, *dst_pos);
    *dst_pos += len;
}
