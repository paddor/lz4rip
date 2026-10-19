use crate::fastcpy::slice_copy;

/// Trait for output buffers used during compression and decompression.
///
/// # Invariant
///
/// `pos() <= capacity()` must hold at all times. `output_mut_with_pos`
/// exposes `&mut usize` for the position; callers must not set it past the
/// buffer length. The decompressor asserts this at entry but cannot guard
/// against mid-call violations from concurrent access.
pub trait Sink {
    /// Pushes a byte to the end of the Sink.
    fn push(&mut self, byte: u8);

    /// Current write position.
    fn pos(&self) -> usize;

    /// Total capacity of the underlying buffer.
    fn capacity(&self) -> usize;

    /// Extends the Sink with `data`.
    fn extend_from_slice(&mut self, data: &[u8]);

    /// Extends the Sink with `data`, but only advances `pos` by `copy_len`.
    /// Allows overcopying into the trailing slack for wildcopy.
    fn extend_from_slice_wild(&mut self, data: &[u8], copy_len: usize);

    /// Copies `num_bytes` from `start` within the output buffer, handling
    /// overlapping (periodic) patterns byte by byte.
    fn extend_from_within_overlapping(&mut self, start: usize, num_bytes: usize);

    /// Returns the underlying buffer and a mutable reference to the position.
    fn output_mut_with_pos(&mut self) -> (&mut [u8], &mut usize);
}

/// `SliceSink` writes into a preallocated `&mut [u8]`.
///
/// # Handling of Capacity
/// Extend methods will panic if there's insufficient capacity left in the Sink.
///
/// # Invariants
///   - Bytes `[..pos()]` are always initialized.
pub struct SliceSink<'a> {
    output: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceSink<'a> {
    /// Creates a `Sink` backed by the given byte slice.
    /// `pos` defines the initial output position in the Sink.
    /// # Panics
    /// Panics if `pos` is out of bounds.
    #[inline]
    pub fn new(output: &'a mut [u8], pos: usize) -> Self {
        let _ = &mut output[..pos]; // bounds check pos
        SliceSink { output, pos }
    }
}

impl Sink for SliceSink<'_> {
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
        self.extend_from_slice_wild(data, data.len());
    }

    #[inline]
    fn extend_from_slice_wild(&mut self, data: &[u8], copy_len: usize) {
        assert!(copy_len <= data.len());
        slice_copy(data, &mut self.output[self.pos..(self.pos) + data.len()]);
        self.pos += copy_len;
    }

    #[inline]
    #[cfg_attr(feature = "nightly", optimize(size))]
    fn extend_from_within_overlapping(&mut self, start: usize, num_bytes: usize) {
        let offset = self.pos - start;
        if offset == 1 {
            let val = self.output[start];
            self.output[self.pos..self.pos + num_bytes].fill(val);
        } else {
            // Seed at most `num_bytes` bytes. When a dict-spanning match leaves
            // an output remainder shorter than `offset`, copying a full
            // `offset`-byte chunk would overshoot the output buffer even though
            // the caller's `pos + num_bytes <= capacity` check held.
            let initial = offset.min(num_bytes);
            self.output.copy_within(start..start + initial, self.pos);
            let mut copied = initial;
            while copied < num_bytes {
                let n = copied.min(num_bytes - copied);
                let src = self.pos;
                self.output.copy_within(src..src + n, src + copied);
                copied += n;
            }
        }
        self.pos += num_bytes;
    }

    #[inline]
    fn output_mut_with_pos(&mut self) -> (&mut [u8], &mut usize) {
        (self.output, &mut self.pos)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_sink_slice() {
        use crate::sink::Sink;
        use crate::sink::SliceSink;
        let mut data = vec![0; 5];
        let sink = SliceSink::new(&mut data, 1);
        assert_eq!(sink.pos(), 1);
        assert_eq!(sink.capacity(), 5);
    }
}
