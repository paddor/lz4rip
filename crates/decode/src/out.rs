//! Decoder output buffer.

use lz4rip_core::slice_copy;

/// Output buffer and write position for the decoder.
///
/// # Invariants
///
/// - `pos <= output.len()`. `decompress_internal` asserts this at entry.
/// - Bytes `[..pos]` are initialized.
pub(crate) struct OutSink<'a> {
    output: &'a mut [u8],
    pos: usize,
}

impl<'a> OutSink<'a> {
    /// Creates a sink that writes from `pos`. Bytes `output[..pos]` must be
    /// initialized; they serve as match history.
    #[inline]
    pub(crate) fn new(output: &'a mut [u8], pos: usize) -> Self {
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
    pub(crate) fn output_mut_with_pos(&mut self) -> (&mut [u8], &mut usize) {
        (self.output, &mut self.pos)
    }

    /// Appends `data`. Panics if it does not fit.
    #[inline]
    pub(crate) fn extend_from_slice(&mut self, data: &[u8]) {
        slice_copy(data, &mut self.output[self.pos..self.pos + data.len()]);
        self.pos += data.len();
    }
}
