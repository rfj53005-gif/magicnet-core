//! Rust1：现有的路径拼接 + `std::fs` 扫描实现。行为与已发布 Rust1 完全一致，
//! 作为默认构建与对照基线保留。

mod encode;
mod scan;

pub(crate) use scan::scan;
