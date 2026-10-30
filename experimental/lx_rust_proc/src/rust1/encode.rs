//! Rust1 的 MPS1 编码：inode → Vec<路径 ID>。

use std::io;

use super::scan::Snapshot;
use crate::blob::{Blob, MAGIC, checked_size};

pub(crate) fn encode(snapshot: Snapshot) -> io::Result<Box<[u8]>> {
    let mut size = checked_size(12, 0)?;
    for executable in &snapshot.executables {
        size = checked_size(size, 4 + executable.len())?;
    }
    for owners in snapshot.entries.values() {
        size = checked_size(size, 8 + owners.len() * 4)?;
    }
    let mut blob = Blob::with_capacity(size);
    blob.u32(MAGIC);
    blob.u32(snapshot.executables.len() as u32);
    blob.u32(snapshot.entries.len() as u32);
    for executable in &snapshot.executables {
        blob.u32(executable.len() as u32);
        blob.bytes(executable);
    }
    for (inode, owners) in &snapshot.entries {
        blob.u32(*inode);
        blob.u32(owners.len() as u32);
        for id in owners {
            blob.u32(*id);
        }
    }
    Ok(blob.finish())
}
