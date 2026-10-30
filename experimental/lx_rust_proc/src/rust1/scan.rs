//! Rust1 扫描：枚举 `/proc`，按 UID 过滤，读取 `exe` 与 `fd/*`，生成 inode → 路径 ID 表。
//!
//! 这是已发布 Rust1 的逻辑搬迁，除模块边界外不做行为改动：绝对路径拼接、目录项
//! 完整收集、`std::fs` 迭代器与 Vec<u32> 持有者表都保持不变。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::blob::{ALL_USERS, MAX_BLOB, overflow};
use crate::proc::{ignorable, pid_name, socket_inode};

#[derive(Default)]
pub(crate) struct Snapshot {
    pub(crate) executables: Vec<Vec<u8>>,
    pub(crate) entries: HashMap<u32, Vec<u32>>,
}

fn read_link(path: &mut Vec<u8>, buffer: &mut [u8]) -> io::Result<usize> {
    path.push(0);
    // SAFETY: path is NUL-terminated, contains no interior NUL, and buffer
    // supplies exactly the writable length passed to libc. Neither is retained.
    let result = unsafe {
        libc::readlink(
            path.as_ptr().cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    let error = if result < 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    path.pop();
    match error {
        Some(error) => Err(error),
        None => Ok(result as usize),
    }
}

fn read_executable(path: &mut Vec<u8>, buffer: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    loop {
        let n = read_link(path, buffer)?;
        if n < buffer.len() {
            return Ok(buffer[..n].to_vec());
        }
        let new_size = buffer
            .len()
            .checked_mul(2)
            .filter(|&n| n <= MAX_BLOB)
            .ok_or_else(overflow)?;
        buffer.resize(new_size, 0);
    }
}

pub(crate) fn snapshot(root: &[u8], uid: u32) -> io::Result<Snapshot> {
    if root.contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let root_path = Path::new(std::ffi::OsStr::from_bytes(root));
    let mut processes = fs::read_dir(root_path)?.collect::<io::Result<Vec<_>>>()?;
    processes.sort_by_cached_key(|entry| entry.file_name());
    let mut result = Snapshot::default();
    let mut ids: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut path = Vec::with_capacity(root.len() + 64);
    path.extend_from_slice(root);
    if !root.ends_with(b"/") {
        path.push(b'/');
    }
    let root_len = path.len();
    let mut fd_link = vec![0u8; libc::PATH_MAX as usize];
    let mut exe_link = vec![0u8; 128];

    for process in processes {
        let name = process.file_name();
        if !pid_name(name.as_bytes()) || !process.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let metadata = match process.metadata() {
            Ok(metadata) => metadata,
            Err(error) if ignorable(&error) => continue,
            Err(error) => return Err(error),
        };
        if uid != ALL_USERS && metadata.uid() != uid {
            continue;
        }
        path.truncate(root_len);
        path.extend_from_slice(name.as_bytes());
        let process_len = path.len();
        path.extend_from_slice(b"/exe");
        let executable = match read_executable(&mut path, &mut exe_link) {
            Ok(executable) => executable,
            Err(error) if ignorable(&error) => continue,
            Err(error) => return Err(error),
        };
        path.truncate(process_len);
        path.extend_from_slice(b"/fd");
        let descriptors = match fs::read_dir(Path::new(std::ffi::OsStr::from_bytes(&path))) {
            Ok(entries) => match entries.collect::<io::Result<Vec<_>>>() {
                Ok(entries) => entries,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        path.push(b'/');
        let fd_len = path.len();
        let mut executable_id = None;
        for descriptor in descriptors {
            path.truncate(fd_len);
            path.extend_from_slice(descriptor.file_name().as_bytes());
            let n = match read_link(&mut path, &mut fd_link) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let inode = match socket_inode(&fd_link[..n]) {
                Some(inode) => inode,
                None => continue,
            };
            let id = *executable_id.get_or_insert_with(|| {
                if let Some(&id) = ids.get(&executable) {
                    id
                } else {
                    let id = result.executables.len() as u32;
                    ids.insert(executable.clone(), id);
                    result.executables.push(executable.clone());
                    id
                }
            });
            let owners = result.entries.entry(inode).or_default();
            if !owners.contains(&id) {
                owners.push(id);
            }
        }
    }
    Ok(result)
}

pub(crate) fn scan(root: &[u8], uid: u32) -> io::Result<Box<[u8]>> {
    super::encode::encode(snapshot(root, uid)?)
}
