//! Allocation accounting for synchronous resource regressions in this crate's
//! test executable only. Per-thread state excludes parallel test activity.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Allocations {
    pub bytes: usize,
    pub calls: usize,
}

thread_local! {
    static COUNT: Cell<Option<Allocations>> = const { Cell::new(None) };
}

struct CountingSystem;

#[global_allocator]
static ALLOCATOR: CountingSystem = CountingSystem;

fn record(bytes: usize) {
    // TLS may already be unavailable when a test thread is being destroyed.
    let _ = COUNT.try_with(|count| {
        if let Some(mut value) = count.get() {
            value.bytes = value.bytes.saturating_add(bytes);
            value.calls = value.calls.saturating_add(1);
            count.set(Some(value));
        }
    });
}

unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's exact GlobalAlloc contract to System.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forward the caller's exact GlobalAlloc contract to System.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size());
        }
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: forward the caller's exact allocation and layout to System.
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            record(size);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forward the original pointer and layout unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        let _ = COUNT.try_with(|count| count.set(None));
    }
}

/// Count allocation calls and requested bytes, including realloc's new size;
/// this is aggregate allocation traffic, not peak resident or live memory.
pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, Allocations) {
    COUNT.with(|count| {
        assert!(
            count.replace(Some(Allocations::default())).is_none(),
            "nested allocation measurement"
        )
    });
    let _reset = Reset;
    let output = operation();
    let allocations = COUNT.with(|count| count.get().expect("measurement is active"));
    (output, allocations)
}
