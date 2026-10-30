//! 目录访问的 RAII 封装：目录 FD、`readdir` 流、`fstatat` / `readlinkat`，
//! 以及单次扫描内复用的 PID 名称 arena。
//!
//! 这里只做句柄与生命周期管理；扫描策略在 `rust2` 模块。所有句柄在 Drop 时关闭，
//! 因此错误路径不会泄漏 FD。`readdir` 使用每次扫描独立的 DIR 句柄，不共享静态缓冲；
//! 目录项名称只在下一次 `next` 前有效，借用检查器会阻止跨项保留。

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

fn path_cstring(path: &[u8]) -> io::Result<CString> {
    CString::new(path).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

// bionic 暴露 `__errno()`，glibc/musl 暴露 `__errno_location()`；libc crate 只为
// Android 声明了前者，因此 Linux 目标在这里补一个同名声明。
#[cfg(not(target_os = "android"))]
unsafe extern "C" {
    fn __errno_location() -> *mut libc::c_int;
}

#[inline]
fn errno_location() -> *mut libc::c_int {
    #[cfg(target_os = "android")]
    // SAFETY: bionic 返回当前线程有效的 errno 指针。
    unsafe {
        libc::__errno()
    }
    #[cfg(not(target_os = "android"))]
    // SAFETY: glibc/musl 返回当前线程有效的 errno 指针。
    unsafe {
        __errno_location()
    }
}

/// 已打开的目录描述符；Drop 时关闭。
pub(crate) struct DirFd {
    fd: OwnedFd,
}

impl DirFd {
    /// 以目录身份打开绝对路径（`/proc` 或测试夹具根）。含 NUL 的路径按 EINVAL 拒绝。
    pub(crate) fn open(path: &[u8]) -> io::Result<Self> {
        let name = path_cstring(path)?;
        // SAFETY: name 是有效 NUL 结尾字符串，flags 无副作用；fd 由 OwnedFd 接管。
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd 是新打开且唯一的描述符。
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// 相对本目录打开子目录（`/proc/<pid>`、`/proc/<pid>/fd`）。
    pub(crate) fn open_at(&self, name: &CStr) -> io::Result<Self> {
        // SAFETY: 目录 FD 有效，name 是 NUL 结尾的名字。
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd 是新打开且唯一的描述符。
        Ok(Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        })
    }

    /// `lstat` 语义的元数据查询（不跟随符号链接，与 Rust1 的 `DirEntry::metadata` 一致）。
    pub(crate) fn stat_at(&self, name: &CStr) -> io::Result<libc::stat> {
        // SAFETY: stat 是 POD，fstatat 会完整写入后再读。
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: 目录 FD 与 name 有效，缓冲区指向可写的 stat。
        let result = unsafe {
            libc::fstatat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(stat)
    }

    /// 相对本目录读取符号链接（`exe`、`fd/<n>`）。
    pub(crate) fn readlink_at(&self, name: &CStr, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: 目录 FD 有效，缓冲区长度与指针一致，返回值在调用后检查。
        let result = unsafe {
            libc::readlinkat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(result as usize)
    }

    /// 把 FD 交给 `fdopendir`，转为可迭代的目录流。
    pub(crate) fn into_stream(self) -> io::Result<DirStream> {
        let fd = self.fd.into_raw_fd();
        // SAFETY: fd 有效且由本对象独占；fdopendir 成功时接管 fd。
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir 失败时 fd 仍归调用方。
            unsafe { libc::close(fd) };
            return Err(error);
        }
        Ok(DirStream { dir })
    }
}

/// `readdir` 流；Drop 时 `closedir`。
pub(crate) struct DirStream {
    dir: *mut libc::DIR,
}

impl DirStream {
    /// 下一项；目录读完返回 `None`，`readdir` 报告错误时返回 `Err`。
    pub(crate) fn next(&mut self) -> io::Result<Option<&CStr>> {
        // SAFETY: dir 由 fdopendir 返回且由 self 独占；名称属于 DIR 缓冲区，
        // 生命周期通过 `&mut self` 借用约束，调用方无法跨项保留。
        unsafe {
            *errno_location() = 0;
            let entry = libc::readdir(self.dir);
            if entry.is_null() {
                let errno = *errno_location();
                if errno == 0 {
                    return Ok(None);
                }
                return Err(io::Error::from_raw_os_error(errno));
            }
            Ok(Some(CStr::from_ptr((*entry).d_name.as_ptr())))
        }
    }
}

impl Drop for DirStream {
    fn drop(&mut self) {
        // SAFETY: dir 来自一次成功的 fdopendir，且只在这里关闭。
        unsafe { libc::closedir(self.dir) };
    }
}

/// 单次扫描内复用的 PID 名称 arena：一次分配 + 排序索引，避免每个目录项一次分配。
pub(crate) struct NameArena {
    bytes: Vec<u8>,
    spans: Vec<(u32, u32)>,
    order: Vec<u32>,
}

impl NameArena {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            spans: Vec::new(),
            order: Vec::new(),
        }
    }

    /// 记录名称；`.` / `..` 返回 false。名称始终以 NUL 结尾保存。
    pub(crate) fn push(&mut self, name: &CStr) -> bool {
        let raw = name.to_bytes();
        if raw == b"." || raw == b".." {
            return false;
        }
        // 偏移超过 u32 时放弃（/proc 目录项远小于 4 GiB）。
        let Ok(start) = u32::try_from(self.bytes.len()) else {
            return false;
        };
        let Ok(end) = u32::try_from(self.bytes.len() + raw.len() + 1) else {
            return false;
        };
        self.bytes.extend_from_slice(raw);
        self.bytes.push(0);
        self.spans.push((start, end));
        self.order.push((self.spans.len() - 1) as u32);
        true
    }

    /// 按原始字节排序（与 Rust1 的 OsString 顺序一致）。
    pub(crate) fn finish(&mut self) {
        let bytes = &self.bytes;
        let spans = &self.spans;
        self.order.sort_unstable_by(|&left, &right| {
            let (left_start, left_end) = spans[left as usize];
            let (right_start, right_end) = spans[right as usize];
            bytes[left_start as usize..left_end as usize]
                .cmp(&bytes[right_start as usize..right_end as usize])
        });
    }

    /// 排序后的 (NUL 结尾名称, 原始字节)。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&CStr, &[u8])> + '_ {
        self.order.iter().map(|&index| {
            let (start, end) = self.spans[index as usize];
            let full = &self.bytes[start as usize..end as usize];
            // SAFETY: arena 只写入过一次结尾 NUL，且名称内部不含 NUL。
            let name = unsafe { CStr::from_bytes_with_nul_unchecked(full) };
            (name, &full[..full.len() - 1])
        })
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    fn temp_root(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "magicnet-dir-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn readdir_sorted_names_and_links() {
        let root = temp_root("basic");
        fs::create_dir(root.join("20")).unwrap();
        fs::create_dir(root.join("3")).unwrap();
        fs::create_dir(root.join("fd")).unwrap();
        symlink(
            OsStr::from_bytes(b"/app/test\xff"),
            root.join("20").join("exe"),
        )
        .unwrap();
        let dir = DirFd::open(root.as_os_str().as_encoded_bytes()).unwrap();
        let mut stream = dir.into_stream().unwrap();
        let mut names = Vec::new();
        while let Some(name) = stream.next().unwrap() {
            names.push(name.to_bytes().to_vec());
        }
        // readdir 会返回 "." 与 ".."；扫描器由 NameArena 过滤。
        names.retain(|name| name != b"." && name != b"..");
        names.sort();
        assert_eq!(names, vec![b"20".to_vec(), b"3".to_vec(), b"fd".to_vec()]);

        let root_fd = DirFd::open(root.as_os_str().as_encoded_bytes()).unwrap();
        let process = root_fd.open_at(c"20").unwrap();
        let mut buffer = [0u8; 64];
        let length = process.readlink_at(c"exe", &mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"/app/test\xff".as_slice());
        let stat = process.stat_at(c"exe").unwrap();
        assert_ne!(stat.st_mode & libc::S_IFMT, libc::S_IFDIR);
        assert!(root_fd.open_at(c"missing").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn arena_sorts_raw_bytes() {
        let mut arena = NameArena::new();
        for name in [c"10", c"2", c"١٢٣", c"not-a-pid"] {
            assert!(arena.push(name));
        }
        assert!(!arena.push(c"."));
        assert!(!arena.push(c".."));
        arena.finish();
        let names: Vec<&[u8]> = arena.iter().map(|(_, raw)| raw).collect();
        assert_eq!(
            names,
            vec![
                b"10".as_slice(),
                b"2".as_slice(),
                b"not-a-pid".as_slice(),
                "١٢٣".as_bytes()
            ]
        );
        assert_eq!(arena.len(), 4);
        assert_eq!(arena.iter().next().unwrap().0.to_bytes(), b"10");
    }
}
