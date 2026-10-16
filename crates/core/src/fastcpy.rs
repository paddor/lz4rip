//! Fixed-size copy for small slices (up to 32 bytes, 64 with AVX).
//! Uses a double-copy trick: copy the first N and last N bytes,
//! which overlap in the middle. Falls back to `memcpy` for larger sizes.

/// Copy `src` into `dst` using fixed-size double-copy tricks for small sizes.
///
/// Generic so the decoder can copy into `MaybeUninit<u8>` output.
/// Thresholds assume one-byte elements.
#[inline]
pub fn slice_copy<T: Copy>(src: &[T], dst: &mut [T]) {
    #[inline(never)]
    #[cold]
    #[track_caller]
    fn len_mismatch_fail(src_len: usize, dst_len: usize) -> ! {
        panic!(
            "source slice length ({src_len}) does not match destination slice length ({dst_len})"
        );
    }

    if src.len() != dst.len() {
        len_mismatch_fail(src.len(), dst.len());
    }
    let len = src.len();

    if src.is_empty() {
        return;
    }

    if len < 4 {
        short_copy(src, dst);
        return;
    }

    if len < 8 {
        double_copy_trick::<T, 4>(src, dst);
        return;
    }

    if len <= 16 {
        double_copy_trick::<T, 8>(src, dst);
        return;
    }

    if len <= 32 {
        double_copy_trick::<T, 16>(src, dst);
        return;
    }

    #[cfg(target_feature = "avx")]
    {
        if len <= 64 {
            double_copy_trick::<T, 32>(src, dst);
            return;
        }
    }

    dst.copy_from_slice(src);
}

#[allow(clippy::inline_always)]
#[inline(always)]
fn short_copy<T: Copy>(src: &[T], dst: &mut [T]) {
    let len = src.len();

    dst[0] = src[0];
    if len >= 2 {
        double_copy_trick::<T, 2>(src, dst);
    }
}

#[allow(clippy::inline_always)]
#[inline(always)]
fn double_copy_trick<T: Copy, const SIZE: usize>(src: &[T], dst: &mut [T]) {
    dst[0..SIZE].copy_from_slice(&src[0..SIZE]);
    dst[src.len() - SIZE..].copy_from_slice(&src[src.len() - SIZE..]);
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::slice_copy;
    use proptest::prelude::*;
    proptest! {
        #[test]
        fn test_fast_short_slice_copy(left: Vec<u8>) {
            let mut right = vec![0u8; left.len()];
            slice_copy(&left, &mut right);
            prop_assert_eq!(&left, &right);
        }
    }

    #[test]
    fn test_fast_short_slice_copy_edge_cases() {
        for len in 0..(512 * 2) {
            let left = (0..len).map(|i| i as u8).collect::<Vec<_>>();
            let mut right = vec![0u8; len];
            slice_copy(&left, &mut right);
            assert_eq!(left, right);
        }
    }

    #[test]
    fn test_fail2() {
        let left = vec![
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 32,
        ];
        let mut right = vec![0u8; left.len()];
        slice_copy(&left, &mut right);
        assert_eq!(left, right);
    }

    #[test]
    fn test_fail() {
        let left = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut right = vec![0u8; left.len()];
        slice_copy(&left, &mut right);
        assert_eq!(left, right);
    }
}
