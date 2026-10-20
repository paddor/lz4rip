//! LZ4 block decompression.

use core::fmt;

use lz4rip_core::DecompressError;
use lz4rip_core::MINMATCH;
use lz4rip_core::Sink;
use lz4rip_core::SliceSink;

use crate::out::OutSink;
use crate::primitives::{LONG_CHUNK, MATCH_CHUNK, MIN_COPY_OFFSET, WIDE_CHUNK, idx};

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Match length bits in the token (low nibble), as in C lz4.
const ML_BITS: u32 = 4;
/// Match length nibble mask. A full nibble means extension bytes follow.
const ML_MASK: usize = (1 << ML_BITS) - 1;
/// Literal length nibble mask. A full nibble means extension bytes follow.
const RUN_MASK: usize = (1 << (u8::BITS - ML_BITS)) - 1;
/// Length extension byte that means another extension byte follows.
const EXT_MORE: u8 = u8::MAX;
/// Bytes in a token.
const TOKEN_LEN: usize = 1;
/// Bytes in a match offset.
const OFFSET_LEN: usize = 2;
/// Bytes in one length extension.
const EXT_LEN: usize = 1;
/// Longest literal a token encodes without an extension byte.
const SHORT_LITERAL_MAX: usize = RUN_MASK - 1;
/// Longest match a token encodes without an extension byte.
pub(crate) const SHORT_MATCH_MAX: usize = MINMATCH + ML_MASK - 1;
/// Fast-loop input window: a token, the longest short literal, the offset
/// and one match length extension byte.
const FAST_IN: usize = TOKEN_LEN + SHORT_LITERAL_MAX + OFFSET_LEN + EXT_LEN;

// The fast loop's short-literal chunk covers the longest short literal, fits
// in the input window after the token, and fits in the output headroom.
const _: () = assert!(WIDE_CHUNK >= SHORT_LITERAL_MAX);
const _: () = assert!(TOKEN_LEN + WIDE_CHUNK <= FAST_IN);
const _: () = assert!(WIDE_CHUNK <= SHORT_MATCH_MAX);

/// Longest literal run the general path copies in `LONG_CHUNK` steps. Longer
/// runs take one exact copy (`memcpy`), whose wider stores keep the store
/// buffer from filling. The inline loop made 64 KiB blocks of mostly
/// incompressible data 16-40% slower.
const WILD_LITERAL_MAX: usize = 256;

/// Read a variable-length integer in the LZ4 encoding.
#[inline]
pub fn read_integer(input: &[u8], input_pos: &mut usize) -> Result<usize, DecompressError> {
    read_integer_bounded(input, input_pos, usize::MAX)
}

/// Read a variable-length integer, bailing early when the accumulated value
/// exceeds `max`. Bounds CPU time on crafted streams with long runs of 0xFF
/// continuation bytes: the caller passes the output capacity so the loop
/// stops as soon as the value is known to be rejected.
#[inline]
pub(crate) fn read_integer_bounded(
    input: &[u8],
    input_pos: &mut usize,
    max: usize,
) -> Result<usize, DecompressError> {
    let mut n: usize = 0;
    loop {
        let extra: u8 = *input
            .get(*input_pos)
            .ok_or(DecompressError::ExpectedAnotherByte)?;
        *input_pos += EXT_LEN;
        n = n
            .checked_add(extra as usize)
            .ok_or(DecompressError::LiteralOutOfBounds)?;
        if extra != EXT_MORE {
            break;
        }
        if n > max {
            break;
        }
    }
    Ok(n)
}

/// Parse the literal length encoded by `token`.
#[inline]
pub(crate) fn read_literal_length(
    input: &[u8],
    input_pos: &mut usize,
    token: u8,
    max_extension: usize,
) -> Result<usize, DecompressError> {
    let literal_length = (token >> ML_BITS) as usize;
    if literal_length == RUN_MASK {
        literal_length
            .checked_add(read_integer_bounded(input, input_pos, max_extension)?)
            .ok_or(DecompressError::LiteralOutOfBounds)
    } else {
        Ok(literal_length)
    }
}

/// Parse the match length encoded by `token`.
#[inline]
pub(crate) fn read_match_length(
    input: &[u8],
    input_pos: &mut usize,
    token: u8,
    max_extension: usize,
) -> Result<usize, DecompressError> {
    let match_length = MINMATCH + (token & ML_MASK as u8) as usize;
    if match_length == MINMATCH + ML_MASK {
        match_length
            .checked_add(read_integer_bounded(input, input_pos, max_extension)?)
            .ok_or(DecompressError::LiteralOutOfBounds)
    } else {
        Ok(match_length)
    }
}

/// Parse a nonzero match offset.
#[inline]
pub(crate) fn read_offset(input: &[u8], input_pos: &mut usize) -> Result<usize, DecompressError> {
    let offset_bytes = input
        .get(*input_pos..)
        .and_then(|remaining| remaining.get(..OFFSET_LEN))
        .ok_or(DecompressError::ExpectedAnotherByte)?;
    *input_pos += OFFSET_LEN;
    let offset = u16::from_le_bytes(offset_bytes.try_into().unwrap()) as usize;
    if offset == 0 {
        return Err(DecompressError::OffsetZero);
    }
    Ok(offset)
}

#[cfg(target_pointer_width = "32")]
#[inline]
fn has_headroom(pos: usize, len: usize, slack: usize, cap: usize) -> bool {
    pos.checked_add(len)
        .and_then(|end| end.checked_add(slack))
        .is_some_and(|end| end <= cap)
}

#[cfg(not(target_pointer_width = "32"))]
#[inline]
fn has_headroom(pos: usize, len: usize, slack: usize, cap: usize) -> bool {
    pos + len + slack <= cap
}

#[cfg(target_pointer_width = "32")]
#[inline]
fn len_exceeds_capacity(pos: usize, len: usize, cap: usize) -> bool {
    len > cap.saturating_sub(pos)
}

#[cfg(not(target_pointer_width = "32"))]
#[inline]
fn len_exceeds_capacity(pos: usize, len: usize, cap: usize) -> bool {
    pos + len > cap
}

#[cfg(target_pointer_width = "32")]
#[inline]
fn expected_output_size(pos: usize, len: usize) -> usize {
    pos.saturating_add(len)
}

#[cfg(not(target_pointer_width = "32"))]
#[inline]
fn expected_output_size(pos: usize, len: usize) -> usize {
    pos + len
}

#[test]
fn check_token() {
    assert!(!does_token_fit(0xFF));
    assert!(does_token_fit(14));
    assert!(does_token_fit(114));
    assert!(!does_token_fit(0b11110000));
    assert!(does_token_fit(0b10110000));
}

/// Whether the literal AND match lengths both fit in the token nibbles
/// (no variable-length extension needed). This gates the fast path.
///
/// True when the literal nibble < 15, which implies both lengths are short.
#[cfg(test)]
#[inline]
fn does_token_fit(token: u8) -> bool {
    token < 0b11110000
}

/// Decompress `input` into `output`, using `ext_dict` for cross-buffer
/// back-references when `USE_DICT` is true.
///
/// Returns the number of bytes written (decompressed) into `output`.
#[inline]
pub(crate) fn decompress_internal<const USE_DICT: bool>(
    input: &[u8],
    output: &mut OutSink<'_>,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    let (out, pos) = output.output_mut_with_pos();
    decompress_loop::<USE_DICT>(input, out, pos, ext_dict)
}

/// Hot loop. Takes the output slice by value and decodes through a local
/// sink, so the slice and position stay in registers instead of being
/// reloaded from and stored to the caller's sink. Writes the final position
/// back to `pos`, also on error.
#[inline(never)]
fn decompress_loop<const USE_DICT: bool>(
    input: &[u8],
    out: &mut [u8],
    pos: &mut usize,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    let mut sink = OutSink::new(out, *pos);
    let result = decompress_body::<USE_DICT>(input, &mut sink, ext_dict);
    *pos = sink.pos();
    result
}

#[inline(always)]
fn decompress_body<const USE_DICT: bool>(
    input: &[u8],
    output: &mut OutSink<'_>,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    let mut input_pos = 0;
    assert!(
        output.pos() <= output.capacity(),
        "sink position ({}) exceeds capacity ({})",
        output.pos(),
        output.capacity(),
    );
    let initial_output_pos = output.pos();

    // The fast loop indexes with 32-bit positions (`idx`), so it runs only
    // on buffers below 4 GiB. Larger buffers decode on the general path.
    let fast = input.len() <= u32::MAX as usize && output.capacity() <= u32::MAX as usize;

    loop {
        // Fast loop: decodes tokens whose literal and match lengths need at
        // most one extension byte, while both buffers have headroom. Any
        // other token leaves the loop before anything is consumed, and the
        // general path below decodes it.
        //
        // Headroom: the token, a `WIDE_CHUNK` literal wildcopy, the offset and
        // one match length byte fit in `FAST_IN` input bytes. The literal
        // wildcopy and a `SHORT_MATCH_MAX` match wildcopy fit in
        // `literal_length + SHORT_MATCH_MAX` output bytes. With 32-bit
        // positions, these two tests prove the bounds checks of the token,
        // literal and offset.
        if fast {
            let (out, pos) = output.output_mut_with_pos();
            loop {
                let ip = idx(input_pos);
                let op = idx(*pos);
                if ip + FAST_IN > input.len() {
                    break;
                }
                let window: &[u8; FAST_IN] = crate::primitives::chunk(input, ip);
                let token = window[0];
                let literal_length = (token >> ML_BITS) as usize;
                if literal_length == RUN_MASK {
                    // Long literal with a one-byte length extension, then a
                    // short match with offset >= `MATCH_CHUNK`. Needs
                    // `LONG_CHUNK` bytes of headroom past the literal in both
                    // buffers: they cover the literal wildcopy, the offset
                    // and the match.
                    let extra = window[TOKEN_LEN];
                    if extra == EXT_MORE {
                        // 270+ literals: decoded here only as the last
                        // sequence (literals only), as in incompressible
                        // messages. Anything else, including a bad length,
                        // goes to the general path.
                        let mut start = ip + TOKEN_LEN;
                        if let Ok(n) = read_integer_bounded(input, &mut start, out.len()) {
                            let literal_length = RUN_MASK + n;
                            if start + literal_length == input.len()
                                && op + literal_length <= out.len()
                            {
                                let mut end = op;
                                crate::primitives::copy_from_src(
                                    input,
                                    start,
                                    out,
                                    &mut end,
                                    literal_length,
                                );
                                *pos = end;
                                return Ok(end - initial_output_pos);
                            }
                        }
                        break;
                    }
                    let literal_length = RUN_MASK + extra as usize;
                    let literal_start = ip + TOKEN_LEN + EXT_LEN;
                    let literal_end = literal_start + literal_length;
                    if literal_end == input.len() && op + literal_length <= out.len() {
                        // Last sequence: literals only. Common for small
                        // incompressible messages.
                        let mut end = op;
                        crate::primitives::copy_from_src(
                            input,
                            literal_start,
                            out,
                            &mut end,
                            literal_length,
                        );
                        *pos = end;
                        return Ok(end - initial_output_pos);
                    }
                    if literal_end + LONG_CHUNK > input.len()
                        || op + literal_length + LONG_CHUNK > out.len()
                    {
                        break;
                    }
                    let offset_bytes: &[u8; OFFSET_LEN] =
                        crate::primitives::chunk(input, literal_end);
                    let offset = u16::from_le_bytes(*offset_bytes) as usize;
                    let match_nib = (token & ML_MASK as u8) as usize;
                    let dst = op + literal_length;
                    if match_nib == ML_MASK || offset < MATCH_CHUNK || offset > dst {
                        break;
                    }
                    crate::primitives::copy_literals_32(
                        input,
                        literal_start,
                        out,
                        op,
                        literal_length,
                    );
                    let mut end = dst;
                    crate::primitives::wild_match_copy_18(
                        out,
                        &mut end,
                        offset,
                        MINMATCH + match_nib,
                    );
                    *pos = end;
                    input_pos = literal_end + OFFSET_LEN;
                    continue;
                }
                if op + literal_length + SHORT_MATCH_MAX > out.len() {
                    break;
                }
                let offset = u16::from_le_bytes(*crate::primitives::chunk(
                    window,
                    TOKEN_LEN + literal_length,
                )) as usize;
                let dst = op + literal_length;
                let match_nib = (token & ML_MASK as u8) as usize;
                if match_nib != ML_MASK {
                    let match_length = MINMATCH + match_nib;
                    if offset >= MATCH_CHUNK && offset <= dst {
                        crate::primitives::store(
                            out,
                            op,
                            *crate::primitives::chunk::<u8, WIDE_CHUNK>(window, TOKEN_LEN),
                        );
                        let mut end = dst;
                        crate::primitives::wild_match_copy_18(out, &mut end, offset, match_length);
                        *pos = end;
                        input_pos = ip + TOKEN_LEN + OFFSET_LEN + literal_length;
                        continue;
                    }
                    if USE_DICT && offset > dst {
                        // The whole match wildcopy comes from the dictionary.
                        let back = offset - dst;
                        if back >= SHORT_MATCH_MAX && back <= ext_dict.len() {
                            crate::primitives::store(
                                out,
                                op,
                                *crate::primitives::chunk::<u8, WIDE_CHUNK>(window, TOKEN_LEN),
                            );
                            let from: &[u8; SHORT_MATCH_MAX] =
                                crate::primitives::chunk(ext_dict, ext_dict.len() - back);
                            crate::primitives::store(out, dst, *from);
                            *pos = dst + match_length;
                            input_pos = ip + TOKEN_LEN + OFFSET_LEN + literal_length;
                            continue;
                        }
                    }
                    break;
                }
                // Long match with a one-byte length extension, offset >=
                // `WIDE_CHUNK`: `LONG_CHUNK` wildcopy in `WIDE_CHUNK` steps.
                let extra = window[TOKEN_LEN + OFFSET_LEN + literal_length];
                if extra == EXT_MORE || offset < WIDE_CHUNK || offset > dst {
                    break;
                }
                let match_length = MINMATCH + ML_MASK + extra as usize;
                if dst + match_length + LONG_CHUNK > out.len() {
                    break;
                }
                crate::primitives::store(
                    out,
                    op,
                    *crate::primitives::chunk::<u8, WIDE_CHUNK>(window, TOKEN_LEN),
                );
                crate::primitives::wild_copy_match_long(out, dst, offset, match_length);
                *pos = dst + match_length;
                input_pos = ip + TOKEN_LEN + OFFSET_LEN + EXT_LEN + literal_length;
            }
        }

        // General path: one token of any kind, with checked reads.
        let token = *input
            .get(input_pos)
            .ok_or(DecompressError::ExpectedAnotherByte)?;
        input_pos += TOKEN_LEN;

        let literal_length = read_literal_length(input, &mut input_pos, token, output.capacity())?;
        if literal_length != 0 {
            if len_exceeds_capacity(input_pos, literal_length, input.len()) {
                return Err(DecompressError::LiteralOutOfBounds);
            }
            if len_exceeds_capacity(output.pos(), literal_length, output.capacity()) {
                return Err(DecompressError::OutputTooSmall {
                    expected: expected_output_size(output.pos(), literal_length),
                    actual: output.capacity(),
                });
            }
            let (out, pos) = output.output_mut_with_pos();
            if literal_length <= WILD_LITERAL_MAX
                && has_headroom(input_pos, literal_length, LONG_CHUNK, input.len())
                && has_headroom(*pos, literal_length, LONG_CHUNK, out.len())
            {
                crate::primitives::wild_copy_literals(input, input_pos, out, pos, literal_length);
            } else {
                crate::primitives::copy_from_src(input, input_pos, out, pos, literal_length);
            }
            input_pos += literal_length;
        }

        if input_pos >= input.len() {
            break;
        }
        let offset = read_offset(input, &mut input_pos)?;

        let mut match_length = read_match_length(input, &mut input_pos, token, output.capacity())?;

        if len_exceeds_capacity(output.pos(), match_length, output.capacity()) {
            return Err(DecompressError::OutputTooSmall {
                expected: expected_output_size(output.pos(), match_length),
                actual: output.capacity(),
            });
        }
        if USE_DICT && offset > output.pos() {
            let copied = copy_from_dict(output, ext_dict, offset, match_length)?;
            if copied == match_length {
                continue;
            }
            match_length -= copied;
        }

        let (out, pos) = output.output_mut_with_pos();
        let (start, did_overflow) = pos.overflowing_sub(offset);
        if did_overflow {
            return Err(DecompressError::OffsetOutOfBounds);
        }
        if offset >= WIDE_CHUNK && has_headroom(*pos, match_length, LONG_CHUNK, out.len()) {
            crate::primitives::wild_copy_match_32(out, start, pos, match_length);
        } else if offset >= WIDE_CHUNK && has_headroom(*pos, match_length, WIDE_CHUNK, out.len()) {
            crate::primitives::wild_copy_match_16(out, start, pos, match_length);
        } else if offset >= MATCH_CHUNK && has_headroom(*pos, match_length, MATCH_CHUNK, out.len())
        {
            crate::primitives::wild_copy_match_8(out, start, pos, match_length);
        } else if (MIN_COPY_OFFSET..MATCH_CHUNK).contains(&offset)
            && has_headroom(*pos, match_length, MATCH_CHUNK, out.len())
        {
            crate::primitives::wild_copy_match_short_offset(out, pos, offset, match_length);
        } else if match_length > offset {
            if offset < MIN_COPY_OFFSET {
                let val = out[start];
                out[*pos..*pos + match_length].fill(val);
                *pos += match_length;
            } else {
                crate::primitives::copy_within_overlapping(out, start, pos, match_length, offset);
            }
        } else {
            crate::primitives::copy_within_nonoverlap(out, start, pos, match_length);
        }
    }
    Ok(output.pos() - initial_output_pos)
}

/// Decompress into a `SliceSink`.
///
/// This is cross-crate plumbing for the frame decoder. It keeps the generic
/// decoder entry point private, so downstream safe code cannot supply an
/// output buffer whose reported capacity disagrees with its slice.
#[inline]
pub fn decompress_into_sink_with_dict<const USE_DICT: bool>(
    input: &[u8],
    output: &mut SliceSink<'_>,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    let (out, pos) = output.output_mut_with_pos();
    let mut sink = OutSink::new(out, *pos);
    let result = decompress_internal::<USE_DICT>(input, &mut sink, ext_dict);
    *pos = sink.pos();
    result
}

#[inline]
fn copy_from_dict(
    output: &mut OutSink<'_>,
    ext_dict: &[u8],
    offset: usize,
    match_length: usize,
) -> Result<usize, DecompressError> {
    debug_assert!(offset > output.pos());
    let (dict_offset, did_overflow) = ext_dict.len().overflowing_sub(offset - output.pos());
    if did_overflow {
        return Err(DecompressError::OffsetOutOfBounds);
    }
    let dict_match_length = match_length.min(ext_dict.len() - dict_offset);
    let ext_match = &ext_dict[dict_offset..dict_offset + dict_match_length];
    output.extend_from_slice(ext_match);
    Ok(dict_match_length)
}

/// Decompress into zero-filled new bytes of `output`, then truncate.
#[cfg(feature = "alloc")]
#[inline]
fn decompress_append_internal<const USE_DICT: bool>(
    input: &[u8],
    output: &mut Vec<u8>,
    max_size: usize,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    output.reserve_exact(max_size);
    let start = output.len();
    output.resize(start + max_size, 0);
    let result = decompress_internal::<USE_DICT>(
        input,
        &mut OutSink::new(&mut output[start..], 0),
        ext_dict,
    );
    output.truncate(start + *result.as_ref().unwrap_or(&0));
    result
}

/// Decompress all bytes of `input` into `output`.
/// `output` should be preallocated with a size of the uncompressed data.
#[inline]
pub fn decompress_into(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
    decompress_internal::<false>(input, &mut OutSink::new(output, 0), b"")
}

/// Decompress all bytes of `input` and append them to `output`.
///
/// Zero-fills `max_size` new bytes at the end of `output`, decodes into them,
/// and truncates to the decoded length. `max_size` must be >= the actual
/// decompressed size. Returns the number of bytes appended. On error,
/// `output` keeps its length and contents.
#[cfg(feature = "alloc")]
#[inline]
pub fn decompress_append(
    input: &[u8],
    output: &mut Vec<u8>,
    max_size: usize,
) -> Result<usize, DecompressError> {
    decompress_append_internal::<false>(input, output, max_size, b"")
}

/// Decompress all bytes of `input` into a new vec.
///
/// `uncompressed_size` must be >= the actual decompressed output size.
#[cfg(feature = "alloc")]
#[inline]
pub fn decompress(input: &[u8], uncompressed_size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut decompressed = Vec::new();
    decompress_append(input, &mut decompressed, uncompressed_size)?;
    Ok(decompressed)
}

/// Decompress `input` into `output` using an external dictionary, returning
/// the number of bytes written.
#[inline]
pub fn decompress_into_with_dict(
    input: &[u8],
    output: &mut [u8],
    dict: &[u8],
) -> Result<usize, DecompressError> {
    decompress_internal::<true>(input, &mut OutSink::new(output, 0), dict)
}

/// A block decompressor that borrows its dictionary.
///
/// This is the no-alloc API. With `alloc`, use
/// [`Decompressor`](crate::Decompressor) instead.
///
/// When no dictionary is needed, use the free functions [`decompress`] or
/// [`decompress_into`] instead.
pub struct DecompressorRef<'a> {
    dict: &'a [u8],
}

impl fmt::Debug for DecompressorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecompressorRef")
            .field("dict_len", &self.dict.len())
            .finish()
    }
}

impl<'a> DecompressorRef<'a> {
    /// Create a decompressor seeded with an external dictionary.
    pub fn with_dict(dict: &'a [u8]) -> Self {
        DecompressorRef { dict }
    }

    /// Decompress `input` into a new `Vec<u8>`.
    ///
    /// `uncompressed_size` must be >= the actual decompressed size.
    #[cfg(feature = "alloc")]
    pub fn decompress(
        &self,
        input: &[u8],
        uncompressed_size: usize,
    ) -> Result<Vec<u8>, DecompressError> {
        let mut decompressed = Vec::new();
        self.decompress_append(input, &mut decompressed, uncompressed_size)?;
        Ok(decompressed)
    }

    /// Decompress `input` into `output`, returning the number of bytes written.
    pub fn decompress_into(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, DecompressError> {
        decompress_internal::<true>(input, &mut OutSink::new(output, 0), self.dict)
    }

    /// Decompress `input` and append it to `output`, returning the number of
    /// bytes appended.
    ///
    /// See [`decompress_append`](crate::decompress_append).
    #[cfg(feature = "alloc")]
    pub fn decompress_append(
        &self,
        input: &[u8],
        output: &mut Vec<u8>,
        max_size: usize,
    ) -> Result<usize, DecompressError> {
        decompress_append_internal::<true>(input, output, max_size, self.dict)
    }
}

/// A block decompressor that owns its dictionary.
///
/// This is the ergonomic API for use with `alloc`. For a no-alloc variant that
/// borrows the dictionary, see [`DecompressorRef`].
///
/// When no dictionary is needed, use the free functions [`decompress`] or
/// [`decompress_into`] instead.
#[cfg(feature = "alloc")]
pub struct Decompressor {
    dict: Vec<u8>,
}

#[cfg(feature = "alloc")]
impl fmt::Debug for Decompressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decompressor")
            .field("dict_len", &self.dict.len())
            .finish()
    }
}

#[cfg(feature = "alloc")]
impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "alloc")]
impl Decompressor {
    /// Create a decompressor with no dictionary.
    pub fn new() -> Self {
        Decompressor { dict: Vec::new() }
    }

    /// Create a decompressor seeded with an external dictionary.
    ///
    /// The dictionary is cloned into owned storage.
    pub fn with_dict(dict: &[u8]) -> Self {
        Decompressor {
            dict: dict.to_vec(),
        }
    }

    /// Decompress `input` into a new `Vec<u8>`.
    ///
    /// `uncompressed_size` must be >= the actual decompressed size.
    pub fn decompress(
        &self,
        input: &[u8],
        uncompressed_size: usize,
    ) -> Result<Vec<u8>, DecompressError> {
        DecompressorRef::with_dict(&self.dict).decompress(input, uncompressed_size)
    }

    /// Decompress `input` into `output`, returning the number of bytes written.
    pub fn decompress_into(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, DecompressError> {
        DecompressorRef::with_dict(&self.dict).decompress_into(input, output)
    }

    /// Decompress `input` and append it to `output`, returning the number of
    /// bytes appended.
    ///
    /// See [`decompress_append`](crate::decompress_append).
    pub fn decompress_append(
        &self,
        input: &[u8],
        output: &mut Vec<u8>,
        max_size: usize,
    ) -> Result<usize, DecompressError> {
        DecompressorRef::with_dict(&self.dict).decompress_append(input, output, max_size)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn all_literal() {
        assert_eq!(decompress(&[0x30, b'a', b'4', b'9'], 3).unwrap(), b"a49");
    }

    #[test]
    fn incomplete_input() {
        assert!(matches!(
            decompress(&[], 255),
            Err(DecompressError::ExpectedAnotherByte)
        ));
        assert!(matches!(
            decompress(&[0xF0], 255),
            Err(DecompressError::ExpectedAnotherByte)
        ));
        assert!(matches!(
            decompress(&[0x0F, 0], 255),
            Err(DecompressError::ExpectedAnotherByte)
        ));
        assert!(matches!(
            decompress(&[0x0F, 1, 0], 255),
            Err(DecompressError::ExpectedAnotherByte)
        ));
    }

    #[test]
    fn offset_oob() {
        assert!(matches!(
            decompress(&[0x40, b'a', 1, 0], 4),
            Err(DecompressError::LiteralOutOfBounds)
        ));
        assert!(matches!(
            decompress(&[0x20, b'a', b'a', 1, 0], 1),
            Err(DecompressError::OutputTooSmall {
                expected: 2,
                actual: 1
            })
        ));
        assert!(matches!(
            decompress(&[0x10, b'a', 1, 0], 4),
            Err(DecompressError::OutputTooSmall {
                expected: 5,
                actual: 4
            })
        ));
        assert!(matches!(
            decompress(
                &[
                    0x0E, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
                ],
                256
            ),
            Err(DecompressError::OffsetOutOfBounds)
        ));
        assert!(matches!(
            DecompressorRef::with_dict(&[0_u8; 250])
                .decompress(&[0x0E, 255, 0, 0x70, 0, 0, 0, 0, 0, 0, 0], 256,),
            Err(DecompressError::OffsetOutOfBounds)
        ));
        assert!(matches!(
            decompress(&[0x0F, 1, 0, 1, 0x70, 0, 0, 0, 0, 0, 0, 0], 256),
            Err(DecompressError::OffsetOutOfBounds)
        ));
        assert!(matches!(
            decompress(&[0x40, 0, 0, 0, 0, 255, 0, 0x70, 0, 0, 0, 0, 0, 0, 0], 256),
            Err(DecompressError::OffsetOutOfBounds)
        ));
    }

    /// Byte-by-byte reference decoder for well-formed blocks.
    fn reference_decode(block: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let token = block[i];
            i += 1;
            let mut lit = (token >> 4) as usize;
            if lit == 15 {
                loop {
                    let b = block[i];
                    i += 1;
                    lit += b as usize;
                    if b != 255 {
                        break;
                    }
                }
            }
            out.extend_from_slice(&block[i..i + lit]);
            i += lit;
            if i == block.len() {
                return out;
            }
            let offset = u16::from_le_bytes([block[i], block[i + 1]]) as usize;
            i += 2;
            let mut len = (token & 0xF) as usize + MINMATCH;
            if len == MINMATCH + 15 {
                loop {
                    let b = block[i];
                    i += 1;
                    len += b as usize;
                    if b != 255 {
                        break;
                    }
                }
            }
            for _ in 0..len {
                out.push(out[out.len() - offset]);
            }
        }
    }

    #[test]
    fn long_overlapping_match_with_offset_16_to_31() {
        for offset in 16..32u16 {
            // 14 literals and a 4-byte match, then 14 more literals and a
            // 70-byte match that overlaps its own output, then 64 trailing
            // literals.
            let mut block = vec![0xE0];
            block.extend(1..=14u8);
            block.extend(14u16.to_le_bytes());
            block.push(0xEF);
            block.extend(21..=34u8);
            block.extend(offset.to_le_bytes());
            block.push(70 - 19);
            block.extend([0xF0, 64 - 15]);
            block.extend((0..64).map(|i| 100 + i as u8));
            let expected = reference_decode(&block);
            assert_eq!(expected.len(), 14 + 4 + 14 + 70 + 64);
            assert_eq!(
                decompress(&block, expected.len()).unwrap(),
                expected,
                "offset {offset}"
            );
        }
    }

    #[test]
    fn offset_0() {
        assert!(matches!(
            decompress(&[0x0E, 0, 0, 0x70, 0, 0, 0, 0, 0, 0, 0], 256),
            Err(DecompressError::OffsetZero)
        ));
    }

    #[test]
    #[should_panic(expected = "sink position")]
    fn corrupted_sink_pos_panics() {
        let input = [0x10, b'A'];
        let mut output = [0u8; 8];
        let mut sink = SliceSink::new(&mut output, 0);
        let (_, pos) = sink.output_mut_with_pos();
        *pos = 9;
        let _ = decompress_into_sink_with_dict::<false>(&input, &mut sink, b"");
    }
}
