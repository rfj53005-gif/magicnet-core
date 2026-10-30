//! Rust2 的 MPS1 编码：持有者表为紧凑结构，输出与 Rust1 完全一致。

use std::io;
use std::rc::Rc;

use super::scan::Snapshot;
use crate::blob::{Blob, MAGIC, checked_size, overflow};

pub(crate) fn encode(snapshot: &Snapshot) -> io::Result<Box<[u8]>> {
    let path_count = u32::try_from(snapshot.executables.len()).map_err(|_| overflow())?;
    let mut size = checked_size(12, 0)?;
    for executable in &snapshot.executables {
        size = checked_size(size, 4 + executable.len())?;
    }
    for (_, _, extras) in snapshot.owners.iter() {
        size = checked_size(size, 8 + (1 + extras.len()) * 4)?;
    }
    let mut blob = Blob::with_capacity(size);
    blob.u32(MAGIC);
    blob.u32(path_count);
    blob.u32(snapshot.owners.len() as u32);
    for executable in &snapshot.executables {
        blob.u32(executable.len() as u32);
        blob.bytes(executable);
    }
    for (inode, first, extras) in snapshot.owners.iter() {
        blob.u32(inode);
        blob.u32(1 + extras.len() as u32);
        blob.u32(first);
        for id in extras {
            blob.u32(*id);
        }
    }
    Ok(blob.finish())
}

// 让 Snapshot 的字段在编码阶段只读借用，避免移动共享的路径字节。
#[allow(dead_code)]
fn _assert_borrow(_: &Rc<[u8]>) {}
