//! Isolated scanner experiment. This library owns every byte it returns.
//! No Go pointer is retained, and no unwind may cross the C ABI boundary.
//!
//! 两种实现共用同一个 C ABI 与 MPS1 线格式：
//! - 默认构建编译 Rust1（现有实现），行为与已发布版本一致。
//! - `rust2` 特性编译 Rust2 候选，规范符号不变。
//! - `bench-dual` 特性把两种实现放进同一个静态库，供基准程序直接交错对比。

use std::io;
use std::slice;

mod blob;
mod dir;
mod proc;
#[cfg(any(not(feature = "rust2"), feature = "bench-dual"))]
mod rust1;
#[cfg(any(feature = "rust2", feature = "bench-dual"))]
mod rust2;
#[cfg(feature = "stats")]
mod stats;
mod unicode_digits;

/// 扫描实现的统一签名：返回完整 MPS1 blob。
type ScanFn = fn(&[u8], u32) -> io::Result<Box<[u8]>>;

#[repr(C)]
pub struct ScanResult {
    pub data: *mut u8,
    pub len: usize,
    pub error: i32,
}

fn failure(error: i32) -> ScanResult {
    ScanResult {
        data: std::ptr::null_mut(),
        len: 0,
        error,
    }
}

/// 校验输入、在 FFI 边界内捕获 panic，并把成功结果的所有权交给调用方。
///
/// # Safety
/// `root` 必须指向 `root_len` 个当次调用内有效的只读字节。
unsafe fn run(scan: ScanFn, root: *const u8, root_len: usize, uid: u32) -> ScanResult {
    if root.is_null() || root_len == 0 || root_len > libc::PATH_MAX as usize {
        return failure(libc::EINVAL);
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: the caller satisfies the documented input lifetime contract.
        let root = unsafe { slice::from_raw_parts(root, root_len) };
        scan(root, uid)
    });
    match result {
        Ok(Ok(blob)) => {
            let len = blob.len();
            let data = Box::into_raw(blob).cast::<u8>();
            ScanResult {
                data,
                len,
                error: 0,
            }
        }
        Ok(Err(error)) => failure(error.raw_os_error().unwrap_or(libc::EIO)),
        Err(_) => failure(-1),
    }
}

/// # Safety
/// `root` must point to `root_len` immutable bytes valid for this call.
#[cfg(not(feature = "rust2"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn magicnet_proc_scan(
    root: *const u8,
    root_len: usize,
    uid: u32,
) -> ScanResult {
    // SAFETY: documented input contract.
    unsafe { run(rust1::scan, root, root_len, uid) }
}

/// # Safety
/// `root` must point to `root_len` immutable bytes valid for this call.
#[cfg(feature = "rust2")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn magicnet_proc_scan(
    root: *const u8,
    root_len: usize,
    uid: u32,
) -> ScanResult {
    // SAFETY: documented input contract.
    unsafe { run(rust2::scan, root, root_len, uid) }
}

/// 基准专用：在同一静态库内直接调用 Rust1 实现。
///
/// # Safety
/// `root` must point to `root_len` immutable bytes valid for this call.
#[cfg(feature = "bench-dual")]
#[unsafe(export_name = "magicnet_proc_scan_r1")]
pub unsafe extern "C" fn magicnet_proc_scan_r1(
    root: *const u8,
    root_len: usize,
    uid: u32,
) -> ScanResult {
    // SAFETY: documented input contract.
    unsafe { run(rust1::scan, root, root_len, uid) }
}

/// 构建期一致性标记：Go 侧用它确认静态库与编译标签匹配。
#[unsafe(no_mangle)]
pub extern "C" fn magicnet_proc_scanner_variant() -> u32 {
    if cfg!(feature = "rust2") { 2 } else { 1 }
}

/// 仅 Rust2 存在的符号，供 `build.sh` 用 nm 检查静态库变体。
#[cfg(feature = "rust2")]
#[unsafe(no_mangle)]
pub extern "C" fn magicnet_proc_scanner_rust2_marker() -> u32 {
    2
}

/// # Safety
/// Consume each successful result exactly once, with its original pointer/len.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn magicnet_proc_free(data: *mut u8, len: usize) {
    if !data.is_null() {
        // SAFETY: pointer and length came from Box<[u8]> in magicnet_proc_scan.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_ffi_input() {
        let result = unsafe { magicnet_proc_scan(std::ptr::null(), 0, 0) };
        assert_eq!(result.error, libc::EINVAL);
        assert!(result.data.is_null());
        assert_eq!(result.len, 0);
        // Freeing a null result must stay a no-op.
        unsafe { magicnet_proc_free(std::ptr::null_mut(), 0) };
    }

    #[test]
    fn variant_matches_features() {
        let expected = if cfg!(feature = "rust2") { 2 } else { 1 };
        assert_eq!(magicnet_proc_scanner_variant(), expected);
    }
}
