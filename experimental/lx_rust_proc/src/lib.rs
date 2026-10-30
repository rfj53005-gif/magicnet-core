//! Isolated scanner experiment. This library owns every byte it returns.
//! No Go pointer is retained, and no unwind may cross the C ABI boundary.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::slice;

mod unicode_digits;

const ALL_USERS: u32 = u32::MAX;
const MAGIC: u32 = 0x3153504d; // "MPS1", little endian.
const MAX_BLOB: usize = 256 * 1024 * 1024;

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

fn ignorable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
}

fn pid_name(name: &[u8]) -> bool {
    if name.iter().all(u8::is_ascii_digit) {
        return true;
    }
    std::str::from_utf8(name).is_ok_and(|name| {
        name.chars().all(|ch| {
            let value = ch as u32;
            unicode_digits::DIGITS.iter().any(|&(lo, hi, step)| {
                value >= lo && value <= hi && (value - lo).is_multiple_of(step)
            })
        })
    })
}

fn socket_inode(link: &[u8]) -> Option<u32> {
    let body = link.strip_prefix(b"socket:[")?.strip_suffix(b"]")?;
    let mut inode = 0u32;
    for &ch in body {
        if !ch.is_ascii_digit() {
            return None;
        }
        inode = inode.checked_mul(10)?.checked_add((ch - b'0') as u32)?;
    }
    // Keep CPU1's empty-body behavior as well as its overflow handling.
    Some(inode)
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
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        buffer.resize(new_size, 0);
    }
}

#[derive(Default)]
struct Snapshot {
    executables: Vec<Vec<u8>>,
    entries: HashMap<u32, Vec<u32>>,
}

fn scan(root: &[u8], uid: u32) -> io::Result<Snapshot> {
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

fn encode(snapshot: Snapshot) -> io::Result<Box<[u8]>> {
    let mut size = 12usize;
    for executable in &snapshot.executables {
        size = size
            .checked_add(4 + executable.len())
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    }
    for owners in snapshot.entries.values() {
        size = size
            .checked_add(8 + owners.len() * 4)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    }
    if size > MAX_BLOB {
        return Err(io::Error::from_raw_os_error(libc::EOVERFLOW));
    }
    let mut blob = Vec::with_capacity(size);
    blob.extend_from_slice(&MAGIC.to_le_bytes());
    blob.extend_from_slice(&(snapshot.executables.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(snapshot.entries.len() as u32).to_le_bytes());
    for executable in snapshot.executables {
        blob.extend_from_slice(&(executable.len() as u32).to_le_bytes());
        blob.extend_from_slice(&executable);
    }
    for (inode, owners) in snapshot.entries {
        blob.extend_from_slice(&inode.to_le_bytes());
        blob.extend_from_slice(&(owners.len() as u32).to_le_bytes());
        for id in owners {
            blob.extend_from_slice(&id.to_le_bytes());
        }
    }
    Ok(blob.into_boxed_slice())
}

/// # Safety
/// `root` must point to `root_len` immutable bytes valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn magicnet_proc_scan(
    root: *const u8,
    root_len: usize,
    uid: u32,
) -> ScanResult {
    if root.is_null() || root_len == 0 || root_len > libc::PATH_MAX as usize {
        return failure(libc::EINVAL);
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: the caller satisfies the documented input lifetime contract.
        let root = unsafe { slice::from_raw_parts(root, root_len) };
        encode(scan(root, uid)?)
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
    fn inode_contract() {
        for (link, expected) in [
            ("socket:[42]", Some(42)),
            ("socket:[]", Some(0)),
            ("socket:[4294967295]", Some(u32::MAX)),
            ("socket:[4294967296]", None),
            ("socket:[18446744073709551616]", None),
            ("socket:[-1]", None),
            ("socket:[1]tail", None),
            ("pipe:[42]", None),
        ] {
            assert_eq!(socket_inode(link.as_bytes()), expected);
        }
    }

    #[test]
    fn pid_contract() {
        assert!(pid_name(b"101"));
        assert!(pid_name("١٢٣".as_bytes()));
        assert!(!pid_name(b"12a"));
        assert!(!pid_name(b"\xff"));
    }

    #[test]
    fn invalid_ffi_input() {
        let result = unsafe { magicnet_proc_scan(std::ptr::null(), 0, 0) };
        assert_eq!(result.error, libc::EINVAL);
        assert!(result.data.is_null());
        assert_eq!(result.len, 0);
    }
}
