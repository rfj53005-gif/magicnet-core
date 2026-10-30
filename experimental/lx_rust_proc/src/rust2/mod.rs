//! Rust2：目录 FD 流式扫描 + 紧凑持有者表。
//!
//! 与 Rust1 的差别只在实现内部：一次枚举 PID 名称、相对目录访问 `exe`/`fd`、
//! 每个进程读完再合并、按需共享可执行路径字节。C ABI 与 MPS1 格式保持不变。

mod encode;
mod scan;
mod store;

pub(crate) use scan::scan;
