//! Decoder output buffer.
//!
//! The default 64-bit build decodes into `[MaybeUninit<u8>]`, so callers can
//! pass uninitialized memory. The decoder stores only bytes copied from the
//! input, the dictionary, or output it already stored, and it loads output
//! bytes only after storing them (or from initialized history before the
//! start position). Every byte below the final position has been stored.
//!
//! Paranoid and 32-bit builds decode into `[u8]` with safe primitives.
//! Uninitialized output is zero-filled first.

use core::mem::MaybeUninit;

use lz4rip_core::slice_copy;

/// Element type of the decoder's output buffer.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
pub(crate) type OutByte = MaybeUninit<u8>;

/// Element type of the decoder's output buffer.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
pub(crate) type OutByte = u8;

/// Views initialized output as decoder output.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline(always)]
pub(crate) fn out_from_init(output: &mut [u8]) -> &mut [OutByte] {
    // SAFETY: `MaybeUninit<u8>` has the layout of `u8`. The decoder never
    // stores an uninitialized byte, so `output` stays initialized.
    unsafe { &mut *(output as *mut [u8] as *mut [MaybeUninit<u8>]) }
}

/// Views initialized output as decoder output.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline(always)]
pub(crate) fn out_from_init(output: &mut [u8]) -> &mut [OutByte] {
    output
}

/// Views input bytes as decoder output elements.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline(always)]
fn as_out_bytes(data: &[u8]) -> &[OutByte] {
    // SAFETY: `MaybeUninit<u8>` has the layout of `u8`, and a shared slice
    // cannot store through it.
    unsafe { &*(data as *const [u8] as *const [MaybeUninit<u8>]) }
}

/// Views input bytes as decoder output elements.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
#[inline(always)]
fn as_out_bytes(data: &[u8]) -> &[OutByte] {
    data
}

/// Views `output[..len]` as initialized bytes.
///
/// # Safety
///
/// The decoder must have stored every byte of `output[..len]`.
#[cfg(not(any(feature = "paranoid", target_pointer_width = "32")))]
#[inline(always)]
pub(crate) unsafe fn assume_init_prefix(output: &mut [OutByte], len: usize) -> &mut [u8] {
    let init = &mut output[..len];
    // SAFETY: the caller guarantees `init` is initialized, and
    // `MaybeUninit<u8>` has the layout of `u8`.
    unsafe { &mut *(init as *mut [MaybeUninit<u8>] as *mut [u8]) }
}

/// Zero-fills `output` and views it as initialized bytes.
///
/// This is the only `unsafe` in paranoid and 32-bit builds. Safe Rust has no
/// stable way to view a filled `[MaybeUninit<u8>]` as `[u8]`.
#[cfg(any(feature = "paranoid", target_pointer_width = "32"))]
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
///
/// `fill` on `[MaybeUninit<u8>]` is an element loop, which LLVM lowers to
/// `memset`. Kani cannot unwind that loop within the harness bounds, so Kani
/// builds stub it with `write_bytes`, which is what `fill` does for `[u8]`.
#[cfg(not(all(kani, not(any(feature = "paranoid", target_pointer_width = "32")))))]
#[inline(always)]
pub(crate) fn fill_out(dst: &mut [OutByte], val: OutByte) {
    dst.fill(val);
}

/// Fills `dst` with `val` (offset-1 matches). Kani stub, see above.
#[cfg(all(kani, not(any(feature = "paranoid", target_pointer_width = "32"))))]
#[inline(always)]
pub(crate) fn fill_out(dst: &mut [OutByte], val: OutByte) {
    // SAFETY: `dst` is writable for `dst.len()` bytes, and Kani harness
    // buffers are zero-initialized.
    unsafe {
        core::ptr::write_bytes(dst.as_mut_ptr().cast::<u8>(), val.assume_init(), dst.len());
    }
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
            as_out_bytes(data),
            &mut self.output[self.pos..self.pos + data.len()],
        );
        self.pos += data.len();
    }

    /// Copies `num_bytes` from `start` within the output, handling
    /// overlapping (periodic) patterns. Panics if it does not fit.
    #[inline]
    #[cfg_attr(feature = "nightly", optimize(size))]
    pub(crate) fn extend_from_within_overlapping(&mut self, start: usize, num_bytes: usize) {
        let offset = self.pos - start;
        if offset == 1 {
            let val = self.output[start];
            fill_out(&mut self.output[self.pos..self.pos + num_bytes], val);
        } else {
            // Seed at most `num_bytes` bytes. When a dict-spanning match leaves
            // an output remainder shorter than `offset`, copying a full
            // `offset`-byte chunk would overshoot the output buffer even though
            // the caller's `pos + num_bytes <= capacity` check held. The unsafe
            // `copy_within_overlapping` clamps the same way (`offset.min(len)`).
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
}
