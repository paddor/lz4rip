#![cfg(feature = "alloc")]
//! `RatioCompressor` makes its 64 KB table only for inputs above 4 KiB, so a
//! compressor per connection for small messages takes 8 KB. `heap_size`
//! reports what each owning compressor holds.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use lz4rip::block::{
    Compressor, Decompressor, DictCompressor, RatioCompressor, RatioDictCompressor, compress_ratio,
    decompress, get_maximum_output_size,
};

/// Tracks the largest request per thread, so tests running in parallel do not
/// see each other's allocations.
struct Tracking;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn record(size: usize) {
    let _ = LARGEST.try_with(|max| max.set(max.get().max(size)));
    live(size as isize);
}

fn live(delta: isize) {
    let _ = LIVE.try_with(|live| live.set(live.get() + delta));
}

fn live_bytes() -> usize {
    LIVE.with(Cell::get) as usize
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
        live(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        live(-(layout.size() as isize));
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

#[test]
fn heap_size_counts_what_compressors_hold() {
    let dict = text(2_000);
    let inputs = [text(100), text(3_000), text(16 * 1024), text(70_000)];
    let mut output = vec![0u8; get_maximum_output_size(70_000)];
    let mut back = vec![0u8; 70_000];
    let start = live_bytes();
    let mut comp = Compressor::new();
    let mut dict_comp = DictCompressor::new(&dict);
    let mut ratio = RatioCompressor::new();
    let mut ratio_dict = RatioDictCompressor::new(&dict);
    let decomp = Decompressor::with_dict(&dict);
    let held = |comp: &Compressor,
                dict_comp: &DictCompressor,
                ratio: &RatioCompressor,
                ratio_dict: &RatioDictCompressor| {
        comp.heap_size()
            + dict_comp.heap_size()
            + ratio.heap_size()
            + ratio_dict.heap_size()
            + decomp.heap_size()
    };
    assert_eq!(
        live_bytes() - start,
        held(&comp, &dict_comp, &ratio, &ratio_dict)
    );
    // A ratio dict compressor holds the dict and makes its tables on first
    // use: 16 KB for inputs up to 4 KiB with the dict, more for larger ones.
    assert_eq!(ratio_dict.heap_size(), 2_000);
    for (i, input) in inputs.iter().enumerate() {
        comp.compress_into(input, &mut output).unwrap();
        dict_comp.compress_into(input, &mut output).unwrap();
        ratio.compress_into(input, &mut output).unwrap();
        let n = ratio_dict.compress_into(input, &mut output).unwrap();
        decomp
            .decompress_into(&output[..n], &mut back[..input.len()])
            .unwrap();
        assert_eq!(
            live_bytes() - start,
            held(&comp, &dict_comp, &ratio, &ratio_dict),
            "after input {i}"
        );
        if i == 0 {
            assert_eq!(ratio.heap_size(), 8 * 1024);
            assert_eq!(ratio_dict.heap_size(), 2_000 + 16 * 1024);
        }
    }
    assert_eq!(ratio.heap_size(), 72 * 1024);
    assert_eq!(decomp.heap_size(), 2_000);
}
