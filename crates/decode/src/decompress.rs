//! LZ4 block decompression.

use core::fmt;

use lz4rip_core::DecompressError;
use lz4rip_core::MINMATCH;
use lz4rip_core::Sink;
use lz4rip_core::SliceSink;

#[cfg(feature = "alloc")]
use alloc::vec;
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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
fn read_integer_bounded(
    input: &[u8],
    input_pos: &mut usize,
    max: usize,
) -> Result<usize, DecompressError> {
    let mut n: usize = 0;
    loop {
        let extra: u8 = *input
            .get(*input_pos)
            .ok_or(DecompressError::ExpectedAnotherByte)?;
        *input_pos += 1;
        n = n
            .checked_add(extra as usize)
            .ok_or(DecompressError::LiteralOutOfBounds)?;
        if extra != 0xFF {
            break;
        }
        if n > max {
            break;
        }
    }
    Ok(n)
}

const LITERAL_LEN_MASK: u8 = 0b1111_0000;

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
pub(crate) fn decompress_internal<const USE_DICT: bool, S: Sink>(
    input: &[u8],
    output: &mut S,
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

    let (lit_margin, match_margin) = (16, 18);
    let safe_input_pos = input.len().saturating_sub(lit_margin + 2);
    let mut safe_output_pos = output.capacity().saturating_sub(lit_margin + match_margin);

    if USE_DICT {
        safe_output_pos = safe_output_pos.saturating_sub(17);
    }

    loop {
        let in_safe_region = input_pos < safe_input_pos;
        let token = if in_safe_region {
            paranoid_unsafe_call!(crate::primitives::read_byte_inbounds(input, input_pos))
        } else {
            *input
                .get(input_pos)
                .ok_or(DecompressError::ExpectedAnotherByte)?
        };
        input_pos += 1;

        let literal_fits = (token & LITERAL_LEN_MASK) != LITERAL_LEN_MASK;
        #[cfg(target_arch = "aarch64")]
        let enter_fast = in_safe_region && output.pos() < safe_output_pos && literal_fits;
        #[cfg(not(target_arch = "aarch64"))]
        let enter_fast = literal_fits && in_safe_region && output.pos() < safe_output_pos;
        #[cfg(feature = "nightly")]
        let enter_fast = core::intrinsics::likely(enter_fast);
        if enter_fast {
            let literal_length = (token >> 4) as usize;
            let match_nib = (token & 0xF) as usize;

            let offset = paranoid_unsafe_call!(crate::primitives::read_u16_inbounds(
                input,
                input_pos + literal_length
            )) as usize;
            if offset == 0 {
                return Err(DecompressError::OffsetZero);
            }

            let (out, pos) = output.output_mut_with_pos();
            paranoid_unsafe_call!(crate::primitives::wild_copy_16(
                input,
                input_pos,
                out,
                pos,
                literal_length
            ));
            input_pos += literal_length + 2;

            if match_nib != 15 {
                let match_length = MINMATCH + match_nib;
                if USE_DICT && offset > *pos {
                    let _ = (out, pos);
                    let copied = copy_from_dict(output, ext_dict, offset, match_length)?;
                    if copied == match_length {
                        continue;
                    }
                    let match_length = match_length - copied;
                    let (start, did_overflow) = output.pos().overflowing_sub(offset);
                    if did_overflow {
                        return Err(DecompressError::OffsetOutOfBounds);
                    }
                    output.extend_from_within_overlapping(start, match_length);
                    continue;
                }

                let (start, did_overflow) = pos.overflowing_sub(offset);
                if did_overflow {
                    return Err(DecompressError::OffsetOutOfBounds);
                }
                if offset >= 8 {
                    paranoid_unsafe_call!(crate::primitives::wild_match_copy_18(
                        out,
                        start,
                        pos,
                        match_length
                    ));
                } else if offset == 1 {
                    let val = out[start];
                    out[*pos..*pos + match_length].fill(val);
                    *pos += match_length;
                } else if match_length <= offset {
                    paranoid_unsafe_call!(crate::primitives::copy_within_nonoverlap(
                        out,
                        start,
                        pos,
                        match_length
                    ));
                } else {
                    paranoid_unsafe_call!(crate::primitives::copy_within_overlapping(
                        out,
                        start,
                        pos,
                        match_length,
                        offset,
                    ));
                }
                continue;
            }

            let match_length = (MINMATCH + 15)
                .checked_add(read_integer_bounded(input, &mut input_pos, out.len())?)
                .ok_or(DecompressError::LiteralOutOfBounds)?;
            if len_exceeds_capacity(*pos, match_length, out.len()) {
                return Err(DecompressError::OutputTooSmall {
                    expected: expected_output_size(*pos, match_length),
                    actual: out.len(),
                });
            }
            if USE_DICT && offset > *pos {
                let _ = (out, pos);
                let copied = copy_from_dict(output, ext_dict, offset, match_length)?;
                if copied == match_length {
                    continue;
                }
                let match_length = match_length - copied;
                let (start, did_overflow) = output.pos().overflowing_sub(offset);
                if did_overflow {
                    return Err(DecompressError::OffsetOutOfBounds);
                }
                output.extend_from_within_overlapping(start, match_length);
                continue;
            }
            let (start, did_overflow) = pos.overflowing_sub(offset);
            if did_overflow {
                return Err(DecompressError::OffsetOutOfBounds);
            }
            if offset >= 32 && has_headroom(*pos, match_length, 32, out.len()) {
                paranoid_unsafe_call!(crate::primitives::wild_copy_match_32(
                    out,
                    start,
                    pos,
                    match_length
                ));
            } else if offset >= 16 && has_headroom(*pos, match_length, 16, out.len()) {
                paranoid_unsafe_call!(crate::primitives::wild_copy_match_16(
                    out,
                    start,
                    pos,
                    match_length
                ));
            } else if offset >= 8 && has_headroom(*pos, match_length, 8, out.len()) {
                paranoid_unsafe_call!(crate::primitives::wild_copy_match_8(
                    out,
                    start,
                    pos,
                    match_length
                ));
            } else if match_length > offset {
                if offset == 1 {
                    let val = out[start];
                    out[*pos..*pos + match_length].fill(val);
                    *pos += match_length;
                } else {
                    paranoid_unsafe_call!(crate::primitives::copy_within_overlapping(
                        out,
                        start,
                        pos,
                        match_length,
                        offset,
                    ));
                }
            } else {
                paranoid_unsafe_call!(crate::primitives::copy_within_nonoverlap(
                    out,
                    start,
                    pos,
                    match_length
                ));
            }
            continue;
        }

        let mut literal_length = (token >> 4) as usize;
        if literal_length != 0 {
            if literal_length == 15 {
                literal_length = literal_length
                    .checked_add(read_integer_bounded(
                        input,
                        &mut input_pos,
                        output.capacity(),
                    )?)
                    .ok_or(DecompressError::LiteralOutOfBounds)?;
            }

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
            if has_headroom(input_pos, literal_length, 32, input.len())
                && has_headroom(*pos, literal_length, 32, out.len())
            {
                paranoid_unsafe_call!(crate::primitives::wild_copy_literals(
                    input,
                    input_pos,
                    out,
                    pos,
                    literal_length
                ));
            } else {
                paranoid_unsafe_call!(crate::primitives::copy_from_src(
                    input,
                    input_pos,
                    out,
                    pos,
                    literal_length
                ));
            }
            input_pos += literal_length;
        }

        if input_pos >= input.len() {
            break;
        }
        let offset = {
            let dst = input
                .get(input_pos..input_pos + 2)
                .ok_or(DecompressError::ExpectedAnotherByte)?;
            input_pos += 2;
            let o = u16::from_le_bytes(dst.try_into().unwrap());
            if o == 0 {
                return Err(DecompressError::OffsetZero);
            }
            o as usize
        };

        let mut match_length = MINMATCH + (token & 0xF) as usize;
        if match_length == MINMATCH + 15 {
            match_length = match_length
                .checked_add(read_integer_bounded(
                    input,
                    &mut input_pos,
                    output.capacity(),
                )?)
                .ok_or(DecompressError::LiteralOutOfBounds)?;
        }

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
        if offset >= 32 && has_headroom(*pos, match_length, 32, out.len()) {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_32(
                out,
                start,
                pos,
                match_length
            ));
        } else if offset >= 16 && has_headroom(*pos, match_length, 16, out.len()) {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_16(
                out,
                start,
                pos,
                match_length
            ));
        } else if offset >= 8 && has_headroom(*pos, match_length, 8, out.len()) {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_8(
                out,
                start,
                pos,
                match_length
            ));
        } else if match_length > offset {
            if offset == 1 {
                let val = out[start];
                out[*pos..*pos + match_length].fill(val);
                *pos += match_length;
            } else {
                paranoid_unsafe_call!(crate::primitives::copy_within_overlapping(
                    out,
                    start,
                    pos,
                    match_length,
                    offset
                ));
            }
        } else {
            paranoid_unsafe_call!(crate::primitives::copy_within_nonoverlap(
                out,
                start,
                pos,
                match_length
            ));
        }
    }
    Ok(output.pos() - initial_output_pos)
}

/// Decompress into a `SliceSink`.
///
/// This is cross-crate plumbing for the frame decoder. It keeps the generic
/// `Sink` entry point private, so downstream safe code cannot supply a `Sink`
/// implementation whose reported capacity disagrees with its output slice.
#[inline]
pub fn decompress_into_sink_with_dict<const USE_DICT: bool>(
    input: &[u8],
    output: &mut SliceSink<'_>,
    ext_dict: &[u8],
) -> Result<usize, DecompressError> {
    decompress_internal::<USE_DICT, _>(input, output, ext_dict)
}

#[inline]
fn copy_from_dict(
    output: &mut impl Sink,
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

/// Decompress all bytes of `input` into `output`.
/// `output` should be preallocated with a size of the uncompressed data.
#[inline]
pub fn decompress_into(input: &[u8], output: &mut [u8]) -> Result<usize, DecompressError> {
    decompress_internal::<false, _>(input, &mut SliceSink::new(output, 0), b"")
}

/// Decompress all bytes of `input` into a new vec.
///
/// `uncompressed_size` must be >= the actual decompressed output size.
#[cfg(feature = "alloc")]
#[inline]
pub fn decompress(input: &[u8], uncompressed_size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut decompressed: Vec<u8> = vec![0; uncompressed_size];
    let decomp_len =
        decompress_internal::<false, _>(input, &mut SliceSink::new(&mut decompressed, 0), b"")?;
    decompressed.truncate(decomp_len);
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
    decompress_internal::<true, _>(input, &mut SliceSink::new(output, 0), dict)
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
        let mut decompressed = vec![0u8; uncompressed_size];
        let len = decompress_internal::<true, _>(
            input,
            &mut SliceSink::new(&mut decompressed, 0),
            self.dict,
        )?;
        decompressed.truncate(len);
        Ok(decompressed)
    }

    /// Decompress `input` into `output`, returning the number of bytes written.
    pub fn decompress_into(
        &self,
        input: &[u8],
        output: &mut [u8],
    ) -> Result<usize, DecompressError> {
        decompress_internal::<true, _>(input, &mut SliceSink::new(output, 0), self.dict)
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

#[cfg(kani)]
mod kani_proofs {
    use super::*;
    use lz4rip_core::MINMATCH;

    // -- End-to-end proofs (slow path, all inputs exhaustive) --

    #[kani::proof]
    #[kani::unwind(5)]
    fn decompress_4byte_no_oob() {
        let input: [u8; 4] = kani::any();
        let mut output = [0u8; 16];
        let mut sink = SliceSink::new(&mut output, 0);
        let _ = decompress_internal::<false, _>(&input, &mut sink, b"");
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn decompress_6byte_no_oob() {
        let input: [u8; 6] = kani::any();
        let mut output = [0u8; 20];
        let mut sink = SliceSink::new(&mut output, 0);
        let _ = decompress_internal::<false, _>(&input, &mut sink, b"");
    }

    #[kani::proof]
    #[kani::unwind(7)]
    fn decompress_dict_6byte_no_oob() {
        let input: [u8; 6] = kani::any();
        let dict: [u8; 8] = kani::any();
        let mut output = [0u8; 20];
        let mut sink = SliceSink::new(&mut output, 0);
        let _ = decompress_internal::<true, _>(&input, &mut sink, &dict);
    }

    // -- Fast-path proofs --
    //
    // The end-to-end proofs above only exercise the slow path (inputs
    // < 19 bytes give safe_input_pos = 0). These proofs model a single
    // fast-path iteration with symbolic indices and token, proving the
    // safe-region margins guarantee every unsafe primitive call is
    // in-bounds.

    /// Pure arithmetic proof: safe-region margins (input.len()-18 for
    /// input, capacity-34 for output) are sufficient for all fast-path
    /// reads and copies, for any buffer sizes 19..=1024.
    #[kani::proof]
    fn fast_path_margins_sufficient() {
        let input_len: usize = kani::any();
        let output_cap: usize = kani::any();
        kani::assume(input_len >= 19 && input_len <= 1024);
        kani::assume(output_cap >= 35 && output_cap <= 1024);

        let safe_input_pos = input_len - 18;
        let safe_output_pos = output_cap - 34;

        let token_pos: usize = kani::any();
        let output_pos: usize = kani::any();
        kani::assume(token_pos < safe_input_pos);
        kani::assume(output_pos < safe_output_pos);

        let token: u8 = kani::any();
        kani::assume((token & 0xF0) != 0xF0);
        let literal_length = (token >> 4) as usize;
        let match_nib = (token & 0xF) as usize;

        let input_pos = token_pos + 1;

        // read_byte_inbounds
        assert!(token_pos < input_len);
        // read_u16_inbounds
        assert!(input_pos + literal_length + 2 <= input_len);
        // wild_copy_16 src
        assert!(input_pos + 16 <= input_len);
        // wild_copy_16 dst
        assert!(output_pos + 16 <= output_cap);
        // literal_length <= 16
        assert!(literal_length <= 16);

        let pos_after = output_pos + literal_length;

        // Short match (match_nib < 15): margins alone guarantee safety
        if match_nib < 15 {
            let match_length = MINMATCH + match_nib;
            // wild_match_copy_18
            assert!(pos_after + 18 <= output_cap);
            assert!(match_length <= 18);
            // copy_within_{nonoverlap,overlapping}
            assert!(pos_after + match_length <= output_cap);
        }
        // Long match (match_nib == 15): match_length is unbounded,
        // but decompress.rs has an explicit bounds check:
        //   if *pos + match_length > out.len() { return Err(...) }
        // So the margins only need to cover wild_copy_16 (the literal),
        // which the assertions above already verify.
    }

    /// Fast-path short match: run actual unsafe primitives on zeroed
    /// buffers with symbolic positions and token. Covers
    /// read_byte_inbounds, read_u16_inbounds, wild_copy_16,
    /// wild_match_copy_18, copy_within_nonoverlap,
    /// copy_within_overlapping.
    #[kani::proof]
    #[kani::unwind(7)]
    fn fast_path_short_match_no_oob() {
        let input = [0u8; 40];
        let mut output = [0u8; 52];

        let token_pos: usize = kani::any();
        let output_pos: usize = kani::any();
        let safe_input_pos = 40 - 18; // 22
        let safe_output_pos = 52 - 34; // 18
        kani::assume(token_pos < safe_input_pos);
        kani::assume(output_pos < safe_output_pos);

        let token: u8 = kani::any();
        kani::assume((token & 0xF0) != 0xF0);

        let input_pos = token_pos + 1;
        let literal_length = (token >> 4) as usize;
        let match_nib = (token & 0xF) as usize;
        kani::assume(match_nib != 15); // short match

        // read_byte_inbounds
        let _ = paranoid_unsafe_call!(crate::primitives::read_byte_inbounds(&input, token_pos));

        // read_u16_inbounds
        let _ = paranoid_unsafe_call!(crate::primitives::read_u16_inbounds(
            &input,
            input_pos + literal_length
        ));

        // wild_copy_16
        let mut pos = output_pos;
        paranoid_unsafe_call!(crate::primitives::wild_copy_16(
            &input,
            input_pos,
            &mut output,
            &mut pos,
            literal_length
        ));

        let match_length = MINMATCH + match_nib;

        let offset: usize = kani::any();
        kani::assume(offset >= 1 && offset <= pos);
        let start = pos - offset;

        if offset >= 8 {
            paranoid_unsafe_call!(crate::primitives::wild_match_copy_18(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        } else if offset == 1 {
            let val = output[start];
            output[pos..pos + match_length].fill(val);
        } else if match_length <= offset {
            paranoid_unsafe_call!(crate::primitives::copy_within_nonoverlap(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        } else {
            paranoid_unsafe_call!(crate::primitives::copy_within_overlapping(
                &mut output,
                start,
                &mut pos,
                match_length,
                offset,
            ));
        }
    }

    /// Fast-path long match: symbolic match_length after the explicit
    /// bounds check, then each wild_copy_match variant and the
    /// overlapping/nonoverlap fallbacks.
    ///
    /// The offset==1 fill path is excluded: it uses safe slice::fill
    /// with no unsafe primitives.
    #[kani::proof]
    #[kani::unwind(10)]
    fn fast_path_long_match_no_oob() {
        let input = [0u8; 40];
        let mut output = [0u8; 64];

        let token_pos: usize = kani::any();
        let output_pos: usize = kani::any();
        let safe_input_pos = 40 - 18; // 22
        let safe_output_pos = 64 - 34; // 30
        kani::assume(token_pos < safe_input_pos);
        kani::assume(output_pos < safe_output_pos);

        let token: u8 = kani::any();
        kani::assume((token & 0xF0) != 0xF0);

        let input_pos = token_pos + 1;
        let literal_length = (token >> 4) as usize;

        let mut pos = output_pos;
        paranoid_unsafe_call!(crate::primitives::wild_copy_16(
            &input,
            input_pos,
            &mut output,
            &mut pos,
            literal_length
        ));

        let match_length: usize = kani::any();
        kani::assume(match_length >= MINMATCH + 15);
        kani::assume(match_length <= 48);

        // Explicit bounds check (mirrors decompress.rs)
        kani::assume(pos + match_length <= output.len());

        let offset: usize = kani::any();
        kani::assume(offset >= 2 && offset <= pos);
        let start = pos - offset;

        if offset >= 32 && pos + match_length + 32 <= output.len() {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_32(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        } else if offset >= 16 && pos + match_length + 16 <= output.len() {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_16(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        } else if offset >= 8 && pos + match_length + 8 <= output.len() {
            paranoid_unsafe_call!(crate::primitives::wild_copy_match_8(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        } else if match_length > offset {
            paranoid_unsafe_call!(crate::primitives::copy_within_overlapping(
                &mut output,
                start,
                &mut pos,
                match_length,
                offset,
            ));
        } else {
            paranoid_unsafe_call!(crate::primitives::copy_within_nonoverlap(
                &mut output,
                start,
                &mut pos,
                match_length
            ));
        }
        let _ = pos;
    }
}
