#![cfg(feature = "alloc")]
//! For sizes from 128 KiB on, Vec-returning decoders allocate no more than
//! the input can decode to, and take a fresh output from `alloc_zeroed`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use lz4rip::block::{
    DecompressError, Decompressor, DecompressorRef, compress, decompress, decompress_append,
    decompress_into,
};

/// Tracks the largest requests per thread, so tests running in parallel do not
/// see each other's allocations.
struct Tracking;

thread_local! {
    static LARGEST: Cell<usize> = const { Cell::new(0) };
    static LARGEST_UNZEROED: Cell<usize> = const { Cell::new(0) };
}

fn record(size: usize, zeroed: bool) {
    let _ = LARGEST.try_with(|max| max.set(max.get().max(size)));
    if !zeroed {
        let _ = LARGEST_UNZEROED.try_with(|max| max.set(max.get().max(size)));
    }
}

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), false);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), true);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size, false);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Tracking = Tracking;

type Decode = fn(&[u8], usize) -> Result<Vec<u8>, DecompressError>;

fn append(input: &[u8], size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    decompress_append(input, &mut out, size).map(|_| out)
}

fn borrowed(input: &[u8], size: usize) -> Result<Vec<u8>, DecompressError> {
    DecompressorRef::with_dict(b"").decompress(input, size)
}

fn borrowed_append(input: &[u8], size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    DecompressorRef::with_dict(b"")
        .decompress_append(input, &mut out, size)
        .map(|_| out)
}

fn owned(input: &[u8], size: usize) -> Result<Vec<u8>, DecompressError> {
    Decompressor::new().decompress(input, size)
}

fn owned_append(input: &[u8], size: usize) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    Decompressor::new()
        .decompress_append(input, &mut out, size)
        .map(|_| out)
}

const DECODERS: [(&str, Decode); 6] = [
    ("decompress", decompress),
    ("decompress_append", append),
    ("DecompressorRef::decompress", borrowed),
    ("DecompressorRef::decompress_append", borrowed_append),
    ("Decompressor::decompress", owned),
    ("Decompressor::decompress_append", owned_append),
];

/// Returns `f`'s result, its largest request, and its largest request for
/// memory that is not zeroed.
fn requests<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    LARGEST.with(|max| max.set(0));
    LARGEST_UNZEROED.with(|max| max.set(0));
    let result = f();
    (
        result,
        LARGEST.with(Cell::get),
        LARGEST_UNZEROED.with(Cell::get),
    )
}

#[test]
fn oversized_limit_allocates_only_what_input_can_decode_to() {
    for input in [&[0x00][..], &[0x10, b'a'], &[0xF0], &[0x00, 0x01, 0x00]] {
        for (name, decode) in DECODERS {
            let (_, largest, _) = requests(|| decode(input, 64 << 20));
            assert!(largest <= 255 * input.len(), "{name}: {largest} bytes");
        }
    }
}

#[test]
fn oversized_limit_keeps_appended_prefix() {
    let mut out = b"prefix".to_vec();
    let (result, largest, _) = requests(|| decompress_append(&[0x10, b'a'], &mut out, 64 << 20));
    assert_eq!(result, Ok(1));
    assert_eq!(out, b"prefixa");
    assert!(largest <= 6 + 255 * 2, "{largest} bytes");
}

#[test]
fn unbounded_limit_is_accepted() {
    for (name, decode) in DECODERS {
        assert_eq!(decode(&[0x10, b'a'], usize::MAX).unwrap(), b"a", "{name}");
        assert_eq!(
            decode(&[], usize::MAX),
            Err(decompress_into(&[], &mut []).unwrap_err()),
            "{name}"
        );
    }
}

#[test]
fn fresh_output_uses_zeroed_allocation() {
    let data = vec![b'a'; 1 << 20];
    let block = compress(&data);
    for (name, decode) in DECODERS {
        let (result, _, unzeroed) = requests(|| decode(&block, data.len()));
        assert_eq!(result.unwrap(), data, "{name}");
        assert!(unzeroed < data.len(), "{name}: {unzeroed} bytes not zeroed");
    }
}

#[test]
fn highest_expansion_fits() {
    // One literal, a match with 4096 full extension bytes, five literals.
    const EXT: usize = 4096;
    let mut block = vec![0x1F, b'a', 1, 0];
    block.extend([0xFF; EXT]);
    block.extend([0, 0x50, b'a', b'a', b'a', b'a', b'a']);
    let expected = vec![b'a'; 1 + 19 + 255 * EXT + 5];
    assert!(expected.len() > 254 * block.len());
    for (name, decode) in DECODERS {
        for size in [expected.len(), usize::MAX] {
            assert_eq!(decode(&block, size).unwrap(), expected, "{name} {size}");
        }
    }
}

#[test]
fn literal_only_block_allocates_its_literals() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let data: Vec<u8> = (0..4096)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 32) as u8
        })
        .collect();
    let block = compress(&data);
    assert_eq!(
        block.len(),
        2 + 16 + data.len(),
        "one literal-only sequence"
    );
    for (name, decode) in DECODERS {
        let (result, largest, _) = requests(|| decode(&block, 1 << 20));
        assert_eq!(result.unwrap(), data, "{name}");
        assert_eq!(largest, data.len(), "{name}");
    }
}
