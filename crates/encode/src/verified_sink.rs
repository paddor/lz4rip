use lz4rip_core::Sink;
use lz4rip_core::slice_copy;

/// Like `SliceSink` but with unchecked writes. The caller must guarantee that all writes fit
/// within the slice (e.g. by verifying `capacity >= get_maximum_output_size(input_len)` before
/// any writes). Used for compression where the capacity invariant is checked upfront by
/// `compress_internal`.
pub(crate) struct VerifiedSliceSink<'a> {
    output: &'a mut [u8],
    pos: usize,
}

impl<'a> VerifiedSliceSink<'a> {
    /// Create a new `VerifiedSliceSink` starting at `pos`.
    #[inline]
    pub(crate) fn new(output: &'a mut [u8], pos: usize) -> Self {
        let _ = &mut output[..pos]; // bounds check pos
        VerifiedSliceSink { output, pos }
    }
}

impl Sink for VerifiedSliceSink<'_> {
    #[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
    #[inline]
    fn push(&mut self, byte: u8) {
        debug_assert!(self.pos < self.output.len());
        // SAFETY: capacity was verified upfront by compress_internal before
        // any writes. pos advances by at most get_maximum_output_size(input_len)
        // which is <= output.len().
        unsafe {
            *self.output.get_unchecked_mut(self.pos) = byte;
        }
        self.pos += 1;
    }

    #[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
    #[inline]
    fn push(&mut self, byte: u8) {
        self.output[self.pos] = byte;
        self.pos += 1;
    }

    #[inline]
    fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.output.len()
    }

    #[inline]
    fn extend_from_slice(&mut self, data: &[u8]) {
        self.extend_from_slice_wild(data, data.len())
    }

    #[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
    #[inline]
    fn extend_from_slice_wild(&mut self, data: &[u8], copy_len: usize) {
        debug_assert!(copy_len <= data.len());
        debug_assert!(self.pos + data.len() <= self.output.len());
        // SAFETY: same upfront capacity guarantee as push().
        let dst = unsafe {
            self.output
                .get_unchecked_mut(self.pos..self.pos + data.len())
        };
        slice_copy(data, dst);
        self.pos += copy_len;
    }

    #[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
    #[inline]
    fn extend_from_slice_wild(&mut self, data: &[u8], copy_len: usize) {
        debug_assert!(copy_len <= data.len());
        let dst = &mut self.output[self.pos..self.pos + data.len()];
        slice_copy(data, dst);
        self.pos += copy_len;
    }

    #[inline]
    #[cfg_attr(feature = "nightly", optimize(size))]
    fn extend_from_within_overlapping(&mut self, start: usize, num_bytes: usize) {
        let offset = self.pos - start;
        for i in start + offset..start + offset + num_bytes {
            self.output[i] = self.output[i - offset];
        }
        self.pos += num_bytes;
    }

    #[inline]
    fn output_mut_with_pos(&mut self) -> (&mut [u8], &mut usize) {
        (self.output, &mut self.pos)
    }
}
