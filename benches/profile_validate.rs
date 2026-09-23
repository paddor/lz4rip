use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lz4rip::block::{compress, decompress_into, validate_block};

const ITERATIONS: usize = 1_000_000;

struct CountingAllocator;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn measure(mut operation: impl FnMut(), iterations: usize) -> (Duration, usize) {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    let elapsed = start.elapsed();
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    (elapsed, allocations)
}

fn report(name: &str, elapsed: Duration, allocations: usize, iterations: usize) {
    let nanos = elapsed.as_nanos() as f64 / iterations as f64;
    println!("{name}: {nanos:.2} ns/iter, {allocations} allocations");
}

fn main() {
    let payload = vec![b'x'; 8 * 1024];
    let compressed = compress(&payload);
    let mut output = vec![0; payload.len()];

    let (elapsed, allocations) = measure(
        || {
            black_box(validate_block(black_box(&compressed), payload.len())).unwrap();
        },
        ITERATIONS,
    );
    report(
        "validate long-match block",
        elapsed,
        allocations,
        ITERATIONS,
    );

    let (elapsed, allocations) = measure(
        || {
            black_box(decompress_into(
                black_box(&compressed),
                black_box(&mut output),
            ))
            .unwrap();
        },
        ITERATIONS,
    );
    report(
        "decompress long-match block",
        elapsed,
        allocations,
        ITERATIONS,
    );

    let rejected_len = 1024;
    let (elapsed, allocations) = measure(
        || {
            assert!(validate_block(black_box(&compressed), rejected_len).is_err());
        },
        ITERATIONS,
    );
    report("validate excessive match", elapsed, allocations, ITERATIONS);

    let mut short_output = vec![0; rejected_len];
    let (elapsed, allocations) = measure(
        || {
            assert!(decompress_into(black_box(&compressed), black_box(&mut short_output)).is_err());
        },
        ITERATIONS,
    );
    report(
        "decompress excessive match",
        elapsed,
        allocations,
        ITERATIONS,
    );
}
