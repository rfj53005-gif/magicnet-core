//! MPS1 线格式的公共常量与编码缓冲。Rust1 与 Rust2 共用同一格式：
//!
//! ```text
//! magic u32 | path count u32 | entry count u32
//! (path len u32 | path bytes) * path count
//! (inode u32 | owner count u32 | path id u32 * owner count) * entry count
//! ```
//!
//! 端序固定为小端；inode、路径 ID 与计数都是 u32。格式本身不变，Rust2 只改变生成过程
//! 与 Go 侧解码时的分配方式。

use std::io;

pub(crate) const ALL_USERS: u32 = u32::MAX;
pub(crate) const MAGIC: u32 = 0x3153504d; // "MPS1", little endian.
pub(crate) const MAX_BLOB: usize = 256 * 1024 * 1024;

pub(crate) fn overflow() -> io::Error {
    io::Error::from_raw_os_error(libc::EOVERFLOW)
}

/// 累加编码大小；超过上限按 EOVERFLOW 拒绝，禁止截断。
pub(crate) fn checked_size(total: usize, add: usize) -> io::Result<usize> {
    total
        .checked_add(add)
        .filter(|&size| size <= MAX_BLOB)
        .ok_or_else(overflow)
}

/// 预分配精确大小的 MPS1 编码缓冲。
pub(crate) struct Blob {
    bytes: Vec<u8>,
}

impl Blob {
    pub(crate) fn with_capacity(size: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(size),
        }
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    pub(crate) fn finish(self) -> Box<[u8]> {
        // 调用方按精确大小预留，因此这里不会再次分配。
        self.bytes.into_boxed_slice()
    }
}
