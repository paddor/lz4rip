#![cfg(feature = "frame")]
//! `FrameDecoder` sizes its buffers and the `read_to_end` output from a
//! frame's declared content size instead of its maximum block size.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{Read, Write};

use lz4rip::frame::{BlockMode, BlockSize, Error, FrameDecoder, FrameEncoder, FrameInfo};

/// Tracks live heap bytes per thread, so tests running in parallel do not
/// see each other's allocations.
struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn track(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        track(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        track(layout.size() as isize);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        track(-(layout.size() as isize));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        track(new_size as isize - layout.size() as isize);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Returns `f`'s result and the most heap memory it held at once.
fn peak_heap<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(base));
    let result = f();
    (result, (PEAK.with(Cell::get) - base) as usize)
}

fn text(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut i = 0u32;
    while out.len() < len {
        out.extend_from_slice(format!("level=INFO service=ingest event={i}\n").as_bytes());
        i += 1;
    }
    out.truncate(len);
    out
}

fn encode(info: FrameInfo, data: &[u8]) -> Vec<u8> {
    let mut encoder = FrameEncoder::with_frame_info(info, Vec::new());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn decode(frame: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    FrameDecoder::new(frame)
        .read_to_end(&mut output)
        .map_err(Error::from)?;
    Ok(output)
}

#[test]
fn read_to_end_reserves_the_declared_content_size() {
    let data = text(300_000);
    for mode in [BlockMode::Independent, BlockMode::Linked] {
        let info = FrameInfo::new()
            .block_size(BlockSize::Max64KB)
            .block_mode(mode)
            .content_size(Some(data.len() as u64));
        let output = decode(&encode(info, &data)).unwrap();
        assert_eq!(output, data);
        assert_eq!(output.capacity(), data.len(), "{mode:?}");
    }
}

#[test]
fn small_frame_with_large_blocks_uses_its_content_size() {
    let data = text(2_000);
    for mode in [BlockMode::Independent, BlockMode::Linked] {
        let info = FrameInfo::new()
            .block_size(BlockSize::Max4MB)
            .block_mode(mode)
            .content_size(Some(data.len() as u64));
        let frame = encode(info, &data);
        let (output, peak) = peak_heap(|| decode(&frame).unwrap());
        assert_eq!(output, data);
        // The output and one block of input and output, far below one 4 MB
        // block.
        assert!(peak < 16 * 1024, "{mode:?}: peak heap {peak} bytes");
    }
}

#[test]
fn blocks_past_the_declared_size_report_the_decoded_length() {
    let data = text(300_000);
    for mode in [BlockMode::Independent, BlockMode::Linked] {
        let info = FrameInfo::new()
            .block_size(BlockSize::Max64KB)
            .block_mode(mode);
        let frame = encode(info.content_size(Some(data.len() as u64)), &data);
        for declared in [1_000u64, 100_000, 299_999, 300_001] {
            // Take the magic, descriptor, content size and header checksum
            // from a frame that declares `declared` bytes.
            let other = encode(info.content_size(Some(declared)), &text(declared as usize));
            let mut lying = frame.clone();
            lying[..15].copy_from_slice(&other[..15]);
            match decode(&lying) {
                Err(Error::ContentLengthError { expected, actual }) => {
                    assert_eq!((expected, actual), (declared, data.len() as u64));
                }
                result => panic!("{mode:?}, declared {declared}: {result:?}"),
            }
        }
    }
}
