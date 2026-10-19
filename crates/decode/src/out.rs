//! Decoder output buffer.
//!
//! The decoder writes into `[u8]`. Uninitialized output is zero-filled first.

use core::mem::MaybeUninit;

use lz4rip_core::slice_copy;

/// Element type of the decoder's output buffer.
pub(crate) type OutByte = u8;

/// Views initialized output as decoder output.
#[inline(always)]
pub(crate) fn out_from_init(output: &mut [u8]) -> &mut [OutByte] {
    output
}

/// Views input bytes as decoder output elements.
#[inline(always)]
pub(crate) fn out_slice(data: &[u8]) -> &[OutByte] {
    data
}

/// Converts an input chunk to decoder output elements.
#[inline(always)]
pub(crate) fn out_array<const N: usize>(chunk: [u8; N]) -> [OutByte; N] {
    chunk
}

/// Zero-fills `output` and views it as initialized bytes.
///
/// This is the only `unsafe` left. Safe Rust has no stable way to view a
/// filled `[MaybeUninit<u8>]` as `[u8]`.
#[allow(unsafe_code)]
#[inline]
pub(crate) fn zero_init(output: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    for byte in output.iter_mut() {
        byte.write(0);
    }
    // SAFETY: the loop above initialized every byte, and `MaybeUninit<u8>`
    // has the layout of `u8`.
    unsafe { &mut *(output as *mut [MaybeUninit<u8>] as *mut [u8]) }
}

/// Fills `dst` with `val` (offset-1 matches).
#[inline(always)]
pub(crate) fn fill_out(dst: &mut [OutByte], val: OutByte) {
    dst.fill(val);
}

/// Output buffer and write position for the decoder.
///
/// # Invariants
///
/// - `pos <= output.len()`. `decompress_internal` asserts this at entry.
/// - Bytes `[..pos]` are initialized.
pub(crate) struct OutSink<'a> {
    output: &'a mut [OutByte],
    pos: usize,
}

impl<'a> OutSink<'a> {
    /// Creates a sink that writes from `pos`. Bytes `output[..pos]` must be
    /// initialized; they serve as match history.
    #[inline]
    pub(crate) fn new(output: &'a mut [OutByte], pos: usize) -> Self {
        OutSink { output, pos }
    }

    /// Current write position.
    #[inline]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Total capacity of the output buffer.
    #[inline]
    pub(crate) fn capacity(&self) -> usize {
        self.output.len()
    }

    /// Returns the output buffer and a mutable reference to the position.
    #[inline]
    pub(crate) fn output_mut_with_pos(&mut self) -> (&mut [OutByte], &mut usize) {
        (self.output, &mut self.pos)
    }

    /// Appends `data`. Panics if it does not fit.
    #[inline]
    pub(crate) fn extend_from_slice(&mut self, data: &[u8]) {
        slice_copy(
            out_slice(data),
            &mut self.output[self.pos..self.pos + data.len()],
        );
        self.pos += data.len();
    }
}
