//! 仅基准程序启用的原生分配计数器（`stats` 特性）。
//!
//! 该分配器只统计本静态库内 Rust 代码的分配；Go 侧分配由基准程序的
//! `runtime.MemStats` 单独统计。生产核心不编译本模块。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: 调用方满足 GlobalAlloc 契约。
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: 调用方满足 GlobalAlloc 契约。
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: 调用方满足 GlobalAlloc 契约。
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        if new_size > layout.size() {
            ALLOCATED_BYTES.fetch_add((new_size - layout.size()) as u64, Ordering::Relaxed);
        }
        // SAFETY: 调用方满足 GlobalAlloc 契约。
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// 自上次 reset 以来的分配次数。
#[unsafe(no_mangle)]
pub extern "C" fn magicnet_proc_alloc_count() -> u64 {
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// 自上次 reset 以来的分配字节数（realloc 只计增长部分）。
#[unsafe(no_mangle)]
pub extern "C" fn magicnet_proc_alloc_bytes() -> u64 {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

/// 清零计数器；基准程序在每次扫描前调用。
#[unsafe(no_mangle)]
pub extern "C" fn magicnet_proc_alloc_reset() {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
}
