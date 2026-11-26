#![cfg(feature = "alloc")]
//! `RatioCompressor` makes its 64 KB table only for inputs above 4 KiB, so a
//! compressor per connection for small messages takes 8 KB.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use lz4rip::block::{RatioCompressor, compress_ratio, decompress, get_maximum_output_size};

/// Tracks the largest request per thread, so tests running in parallel do not
/// see each other's allocations.
struct Tracking;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn record(size: usize) {
    let _ = LARGEST.try_with(|max| max.set(max.get().max(size)));
}

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Tracking = Tracking;

fn largest_since_reset() -> usize {
    LARGEST.with(|max| max.replace(0))
}

fn text(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| b"the quick brown fox jumps over the lazy dog "[i % 44] ^ (i / 997) as u8)
        .collect()
}

#[test]
fn small_inputs_take_only_the_small_table() {
    let inputs: Vec<Vec<u8>> = [64, 256, 1200, 4096].map(text).into();
    let want: Vec<Vec<u8>> = inputs.iter().map(|input| compress_ratio(input)).collect();
    let mut output = vec![0u8; get_maximum_output_size(64 * 1024)];
    largest_since_reset();

    let mut comp = RatioCompressor::new();
    let mut got = Vec::new();
    for input in &inputs {
        let n = comp.compress_into(input, &mut output).unwrap();
        got.push(n);
    }
    assert!(largest_since_reset() <= 8 * 1024);
    for (input, want) in inputs.iter().zip(&want) {
        let n = comp.compress_into(input, &mut output).unwrap();
        assert_eq!(output[..n], want[..]);
    }
    assert_eq!(got, want.iter().map(Vec::len).collect::<Vec<_>>());

    // The first input above 4 KiB makes the 64 KB table, then reuses it.
    let large = text(16 * 1024);
    let n = comp.compress_into(&large, &mut output).unwrap();
    assert_eq!(largest_since_reset(), 64 * 1024);
    assert_eq!(decompress(&output[..n], large.len()).unwrap(), large);
    for input in inputs.iter().chain([&large]) {
        let n = comp.compress_into(input, &mut output).unwrap();
        assert_eq!(output[..n], compress_ratio(input)[..]);
    }
    largest_since_reset();
    comp.compress_into(&large, &mut output).unwrap();
    assert_eq!(largest_since_reset(), 0);
}
