//! Rust2 扫描：`/proc` 目录 FD + `readdir` 流式读取 + `openat`/`readlinkat` 相对访问。
//!
//! 与 Rust1 的可见结果、错误语义完全一致，只改变内部资源管理与数据结构：
//!
//! 1. 打开 root 目录 FD，一次流式枚举全部目录项；名称写入 `NameArena`（单次分配）后
//!    按原始字节排序，与 Rust1 的 `read_dir` + `OsString` 排序等价；
//! 2. 按排序顺序逐个进程：`pid_name` 先筛名称，`stat_at`（`AT_SYMLINK_NOFOLLOW`）再做
//!    目录与 UID 过滤，然后 `open_at` 进程目录并 `readlink_at("exe")`；exe 缓冲在整次
//!    扫描中复用，填满时倍增（上限 `MAX_BLOB`），非 UTF-8 字节原样保留；
//! 3. 只在发现 socket 时才登记路径 ID：`HashMap<Rc<[u8]>, u32>` 按 `exe` 字节去重，
//!    未命中时只复制一次字节，索引与 `executables` 共享同一个 `Rc`；
//! 4. 流式枚举 `fd` 目录，逐项 `readlinkat` + `socket_inode`，把 `(inode, path_id)`
//!    暂存进本进程的小 Vec；只有目录完整读完后才合并进 `OwnerTable`，`readdir` 中途
//!    出错时丢弃整批暂存项（绝不返回某个进程的部分持有者数据）；
//! 5. 扫描结束把 `Snapshot` 交给 MPS1 编码器；目录句柄、缓冲与索引都是函数局部状态，
//!    随返回释放，没有全局锁、静态缓冲或跨扫描缓存。
//!
//! 错误语义与 Rust1 逐分支一致：root 缺失/不是目录、`exe` 是普通文件（EINVAL）等向
//! 调用方报告；进程消失/无权限（`ignorable`）、`fd` 不可读、`exe` 缺失等跳过该进程。
//! 注意 `readlink` 不解析最后一跳，自指软链 `exe -> exe` 返回目标字节而非 ELOOP，
//! 与 Rust1/Go 参考实现一致（另有 bench-dual 对照测试固定该行为）。

use std::collections::HashMap;
use std::io;
use std::rc::Rc;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::blob::{ALL_USERS, MAX_BLOB, overflow};
use crate::dir::{DirFd, NameArena};
use crate::proc::{ignorable, pid_name, socket_inode};

use super::encode::encode;
use super::store::OwnerTable;

/// 读取 `exe` 的初始缓冲大小；与 Rust1 相同，填满时按 2 倍增长。
const EXE_BUFFER_INITIAL: usize = 128;

/// 单次扫描的中间结果；路径 ID 就是 `executables` 的下标。
pub(crate) struct Snapshot {
    pub(crate) executables: Vec<Rc<[u8]>>,
    pub(crate) owners: OwnerTable,
}

/// `readdir` 中途失败的唯一注入点：`usize::MAX` 表示关闭。仅测试构建存在。
///
/// 语义是“一次性”：第一个恰好处理了该数量目录项的进程会看到错误，注入后自动关闭，
/// 这样同一个测试里的后续进程仍能正常读完，用来验证失败进程的暂存被丢弃。
#[cfg(test)]
static FD_READDIR_FAIL_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);

#[cfg(test)]
fn injected_fd_readdir_failure(processed: usize) -> bool {
    FD_READDIR_FAIL_AFTER
        .compare_exchange(processed, usize::MAX, Ordering::SeqCst, Ordering::Relaxed)
        .is_ok()
}

#[cfg(not(test))]
#[inline(always)]
const fn injected_fd_readdir_failure(_processed: usize) -> bool {
    false
}

/// 一个进程 `fd` 目录的枚举结果。
enum FdOwners {
    /// 目录完整读完；暂存项按枚举顺序排列。
    Complete(Vec<(u32, u32)>),
    /// 进程消失、`fd` 不可读或 `readdir` 中途失败：整批暂存丢弃。
    Discard,
}

/// 读取 `exe` 链接并返回有效字节数；缓冲区被填满（结果可能被截断）时倍增后重读。
fn read_executable(process: &DirFd, buffer: &mut Vec<u8>) -> io::Result<usize> {
    loop {
        let length = process.readlink_at(c"exe", buffer)?;
        if length < buffer.len() {
            return Ok(length);
        }
        let doubled = buffer
            .len()
            .checked_mul(2)
            .filter(|&size| size <= MAX_BLOB)
            .ok_or_else(overflow)?;
        buffer.resize(doubled, 0);
    }
}

/// 为 `exe` 取路径 ID：命中索引时复用；未命中时只复制一次字节并共享 `Rc`。
fn intern(
    ids: &mut HashMap<Rc<[u8]>, u32>,
    executables: &mut Vec<Rc<[u8]>>,
    executable: &[u8],
) -> u32 {
    if let Some(&id) = ids.get(executable) {
        return id;
    }
    let path: Rc<[u8]> = Rc::from(executable);
    let id = executables.len() as u32;
    ids.insert(Rc::clone(&path), id);
    executables.push(path);
    id
}

/// 枚举 root 一层的全部目录项名称并按原始字节排序，与 Rust1 的排序顺序一致。
fn list_process_names(root: &[u8]) -> io::Result<NameArena> {
    let directory = DirFd::open(root)?;
    let mut stream = directory.into_stream()?;
    let mut names = NameArena::new();
    while let Some(name) = stream.next()? {
        names.push(name);
    }
    names.finish();
    Ok(names)
}

/// 流式枚举一个进程的 `fd` 目录，把 socket 持有者暂存在本进程的小 Vec 里。
///
/// 打开目录失败与 `readdir` 中途失败都返回 [`FdOwners::Discard`]，调用方丢弃整批
/// 暂存项；这与 Rust1 “先完整 collect 再处理”的可见行为一致，不会产生部分进程数据。
fn collect_fd_owners(
    process: &DirFd,
    executable: &[u8],
    ids: &mut HashMap<Rc<[u8]>, u32>,
    executables: &mut Vec<Rc<[u8]>>,
    link_buffer: &mut [u8],
) -> FdOwners {
    // `into_stream` 会接管目录 FD，因此相对读取链接需要第二个句柄；两者都是 RAII，
    // 进程处理完即关闭，不随 FD 数量增长。
    let readlink_directory = match process.open_at(c"fd") {
        Ok(directory) => directory,
        // Rust1 对 `fs::read_dir(fd)` 的任何错误都是 continue。
        Err(_) => return FdOwners::Discard,
    };
    let stream_directory = match process.open_at(c"fd") {
        Ok(directory) => directory,
        Err(_) => return FdOwners::Discard,
    };
    let mut stream = match stream_directory.into_stream() {
        Ok(stream) => stream,
        Err(_) => return FdOwners::Discard,
    };
    let mut staged: Vec<(u32, u32)> = Vec::new();
    let mut executable_id: Option<u32> = None;
    let mut processed = 0usize;
    loop {
        if injected_fd_readdir_failure(processed) {
            return FdOwners::Discard;
        }
        let name = match stream.next() {
            Ok(Some(name)) => name,
            Ok(None) => break,
            Err(_) => return FdOwners::Discard,
        };
        let raw = name.to_bytes();
        // `std::fs::ReadDir` 不向调用方暴露 `.` / `..`，这里保持同样的可见集合。
        if raw == b"." || raw == b".." {
            continue;
        }
        processed += 1;
        let length = match readlink_directory.readlink_at(name, link_buffer) {
            Ok(length) => length,
            Err(_) => continue,
        };
        let Some(inode) = socket_inode(&link_buffer[..length]) else {
            continue;
        };
        let path_id = *executable_id.get_or_insert_with(|| intern(ids, executables, executable));
        staged.push((inode, path_id));
    }
    FdOwners::Complete(staged)
}

pub(crate) fn scan(root: &[u8], uid: u32) -> io::Result<Box<[u8]>> {
    let names = list_process_names(root)?;
    // `into_stream` 接管了枚举用的目录 FD；后续相对访问需要重新打开 root。
    let directory = DirFd::open(root)?;
    let mut executables: Vec<Rc<[u8]>> = Vec::new();
    let mut owners = OwnerTable::new();
    let mut ids: HashMap<Rc<[u8]>, u32> = HashMap::new();
    // 整次扫描复用；exe 可增长，fd 链接长度受 PATH_MAX 限制（与 Rust1 相同）。
    let mut executable_buffer = vec![0u8; EXE_BUFFER_INITIAL];
    let mut link_buffer = vec![0u8; libc::PATH_MAX as usize];

    for (name, raw) in names.iter() {
        if !pid_name(raw) {
            continue;
        }
        let stat = match directory.stat_at(name) {
            Ok(stat) => stat,
            Err(error) if ignorable(&error) => continue,
            Err(error) => return Err(error),
        };
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            continue;
        }
        if uid != ALL_USERS && stat.st_uid != uid {
            continue;
        }
        let process = match directory.open_at(name) {
            Ok(process) => process,
            Err(error) if ignorable(&error) => continue,
            Err(error) => return Err(error),
        };
        let executable_length = match read_executable(&process, &mut executable_buffer) {
            Ok(length) => length,
            Err(error) if ignorable(&error) => continue,
            Err(error) => return Err(error),
        };
        if let FdOwners::Complete(staged) = collect_fd_owners(
            &process,
            &executable_buffer[..executable_length],
            &mut ids,
            &mut executables,
            &mut link_buffer,
        ) {
            for (inode, path_id) in staged {
                owners.insert(inode, path_id);
            }
        }
    }
    encode(&Snapshot {
        executables,
        owners,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use std::thread;
    use std::time::Duration;

    /// 测试里所有扫描串行执行：注入点与 /proc/self/fd 计数都是进程级状态。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_lock() -> MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 临时夹具根目录，Drop 时清理。
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "magicnet-rust2-{label}-{}-{:?}",
                std::process::id(),
                thread::current().id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn root_bytes(&self) -> &[u8] {
            self.root.as_os_str().as_bytes()
        }

        /// 创建一个进程目录：`<pid>/exe` 软链 + `<pid>/fd/<name>` 软链集合。
        fn process(&self, pid: &str, executable: &[u8], links: &[(&str, &[u8])]) {
            let directory = self.process_dir(pid);
            fs::create_dir_all(directory.join("fd")).unwrap();
            symlink(OsStr::from_bytes(executable), directory.join("exe")).unwrap();
            for (name, target) in links {
                symlink(OsStr::from_bytes(target), directory.join("fd").join(name)).unwrap();
            }
        }

        fn process_dir(&self, pid: &str) -> PathBuf {
            self.root.join(pid)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// 与 Go `fixture()` 等价的夹具：多持有者、非 UTF-8、Unicode PID、长 exe、
    /// PID 目录软链、空目录、无效 socket 链接都在这里出现。
    fn reference_fixture() -> Fixture {
        let fixture = Fixture::new("reference");
        fixture.process(
            "10",
            b"/app/first",
            &[
                ("1", b"socket:[42]"),
                ("2", b"socket:[42]"),
                ("3", b"socket:[4294967295]"),
                ("4", b"socket:[]"),
            ],
        );
        fixture.process("100", b"/app/first", &[("4", b"socket:[42]")]);
        fixture.process("11", b"/app/raw\xff", &[("1", b"socket:[42]")]);
        fixture.process("2", b"/app/second", &[("4", b"socket:[42]")]);
        fixture.process("١٢٣", b"/app/unicode", &[("1", b"socket:[42]")]);
        fixture.process("999", &long_path(701), &[("1", b"socket:[77]")]);
        fixture.process("not-a-pid", b"/app/ignored", &[("1", b"socket:[999]")]);
        fixture.process(
            "300",
            b"/app/empty",
            &[
                ("1", b"socket:[4294967296]"),
                ("2", b"pipe:[42]"),
                ("3", b"socket:[-1]"),
                ("4", b"socket:[2]extra"),
                ("5", b"socket:[18446744073709551616]"),
            ],
        );
        // PID 目录软链必须跳过（不跟随），空目录不产生条目。
        symlink(fixture.process_dir("10"), fixture.path().join("22")).unwrap();
        fs::create_dir(fixture.path().join("500")).unwrap();
        fixture
    }

    fn long_path(length: usize) -> Vec<u8> {
        let mut path = vec![b'/'];
        path.resize(length, b'x');
        path
    }

    fn expected_executables() -> Vec<Vec<u8>> {
        vec![
            b"/app/first".to_vec(),
            b"/app/raw\xff".to_vec(),
            b"/app/second".to_vec(),
            long_path(701),
            b"/app/unicode".to_vec(),
        ]
    }

    fn expected_owners() -> BTreeMap<u32, Vec<Vec<u8>>> {
        BTreeMap::from([
            (0, vec![b"/app/first".to_vec()]),
            (
                42,
                vec![
                    b"/app/first".to_vec(),
                    b"/app/raw\xff".to_vec(),
                    b"/app/second".to_vec(),
                    b"/app/unicode".to_vec(),
                ],
            ),
            (77, vec![long_path(701)]),
            (u32::MAX, vec![b"/app/first".to_vec()]),
        ])
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Decoded {
        executables: Vec<Vec<u8>>,
        owners: BTreeMap<u32, Vec<Vec<u8>>>,
    }

    fn take_u32(blob: &[u8], offset: &mut usize) -> u32 {
        let end = *offset + 4;
        let value = u32::from_le_bytes(blob[*offset..end].try_into().unwrap());
        *offset = end;
        value
    }

    fn decode(blob: &[u8]) -> Decoded {
        let mut offset = 0usize;
        assert_eq!(take_u32(blob, &mut offset), 0x3153_504d, "bad magic");
        let path_count = take_u32(blob, &mut offset) as usize;
        let entry_count = take_u32(blob, &mut offset) as usize;
        let mut executables = Vec::with_capacity(path_count);
        for _ in 0..path_count {
            let length = take_u32(blob, &mut offset) as usize;
            let end = offset + length;
            executables.push(blob[offset..end].to_vec());
            offset = end;
        }
        let mut owners = BTreeMap::new();
        for _ in 0..entry_count {
            let inode = take_u32(blob, &mut offset);
            let count = take_u32(blob, &mut offset) as usize;
            let mut paths = Vec::with_capacity(count);
            for _ in 0..count {
                let id = take_u32(blob, &mut offset) as usize;
                paths.push(executables[id].clone());
            }
            assert!(owners.insert(inode, paths).is_none(), "duplicate inode");
        }
        assert_eq!(offset, blob.len(), "trailing blob bytes");
        Decoded {
            executables,
            owners,
        }
    }

    fn scan_decoded(root: &[u8], uid: u32) -> Decoded {
        decode(&scan(root, uid).expect("scan failed"))
    }

    #[test]
    fn fixture_matches_rust1_reference_semantics() {
        let _guard = test_lock();
        let fixture = reference_fixture();
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(decoded.executables, expected_executables());
        assert_eq!(decoded.owners, expected_owners());
    }

    #[test]
    fn uid_filtering_matches_directory_owner() {
        let _guard = test_lock();
        let fixture = Fixture::new("uid");
        fixture.process("1", b"/app/a", &[("0", b"socket:[11]")]);
        fixture.process("2", b"/app/b", &[("0", b"socket:[22]")]);
        let all_users = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(all_users.owners.len(), 2);
        let owner = unsafe { libc::geteuid() };
        assert_eq!(scan_decoded(fixture.root_bytes(), owner), all_users);
        let other = owner.wrapping_add(1);
        assert_ne!(other, ALL_USERS);
        let filtered = scan_decoded(fixture.root_bytes(), other);
        assert!(filtered.owners.is_empty());
        assert!(filtered.executables.is_empty());
    }

    #[test]
    fn error_contract_missing_root() {
        let _guard = test_lock();
        let fixture = Fixture::new("missing-root");
        let missing = fixture.path().join("missing");
        let error = scan(missing.as_os_str().as_bytes(), ALL_USERS).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOENT));
        // root 是普通文件：与 Rust1 的 `fs::read_dir` 一致，返回 ENOTDIR。
        let file = fixture.path().join("root-file");
        fs::write(&file, b"").unwrap();
        let error = scan(file.as_os_str().as_bytes(), ALL_USERS).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOTDIR));
    }

    #[test]
    fn root_with_nul_is_rejected() {
        let _guard = test_lock();
        let error = scan(b"/proc\0", ALL_USERS).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
    }

    #[test]
    fn error_contract_regular_exe() {
        let _guard = test_lock();
        let fixture = Fixture::new("regular-exe");
        let directory = fixture.process_dir("10");
        fs::create_dir_all(directory.join("fd")).unwrap();
        fs::write(directory.join("exe"), b"").unwrap();
        let error = scan(fixture.root_bytes(), ALL_USERS).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EINVAL));
    }

    /// `readlink` 不解析最后一跳，所以 `exe -> exe` 会原样返回目标字节 "exe"（不产生
    /// ELOOP）；Rust1 与 Go 参考实现同样如此，这里固定与 Rust1 一致的可见行为。
    #[test]
    fn self_referential_exe_matches_rust1_readlink_semantics() {
        let _guard = test_lock();
        let fixture = Fixture::new("loop-exe");
        let directory = fixture.process_dir("10");
        fs::create_dir_all(directory.join("fd")).unwrap();
        symlink("exe", directory.join("exe")).unwrap();
        symlink("socket:[42]", directory.join("fd").join("1")).unwrap();
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(
            decoded.owners,
            BTreeMap::from([(42, vec![b"exe".to_vec()])])
        );
    }

    #[test]
    fn regular_fd_file_is_skipped_without_error() {
        let _guard = test_lock();
        let fixture = Fixture::new("regular-fd");
        let directory = fixture.process_dir("10");
        fs::create_dir_all(&directory).unwrap();
        symlink(OsStr::from_bytes(b"/app/one"), directory.join("exe")).unwrap();
        fs::write(directory.join("fd"), b"").unwrap();
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert!(decoded.owners.is_empty());
        assert!(decoded.executables.is_empty());
    }

    #[test]
    fn pid_directory_symlink_is_skipped() {
        let _guard = test_lock();
        let fixture = Fixture::new("pid-symlink");
        fixture.process("10", b"/app/real", &[("3", b"socket:[42]")]);
        // 目标目录名不是 PID，所以只有 22 若是“跟随软链”才会暴露 inode 43。
        fixture.process("target", b"/app/hidden", &[("3", b"socket:[43]")]);
        symlink(fixture.process_dir("target"), fixture.path().join("22")).unwrap();
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(
            decoded.owners.keys().copied().collect::<Vec<u32>>(),
            vec![42]
        );
        assert_eq!(decoded.owners[&42], vec![b"/app/real".to_vec()]);
    }

    #[test]
    fn missing_exe_skips_process() {
        let _guard = test_lock();
        let fixture = Fixture::new("missing-exe");
        let directory = fixture.process_dir("10");
        fs::create_dir_all(directory.join("fd")).unwrap();
        symlink("socket:[42]", directory.join("fd").join("1")).unwrap();
        // `exe` 缺失是 ENOENT（可忽略错误）：跳过该进程，即使它有 socket。
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert!(decoded.owners.is_empty());
        assert!(decoded.executables.is_empty());
    }

    #[test]
    fn invalid_socket_links_are_ignored_except_empty_body() {
        let _guard = test_lock();
        let fixture = Fixture::new("invalid-links");
        fixture.process(
            "1",
            b"/app/one",
            &[
                ("0", b"socket:[4294967296]"),
                ("1", b"pipe:[7]"),
                ("2", b"socket:[-1]"),
                ("3", b"socket:[2]extra"),
                ("4", b"socket:[]"),
            ],
        );
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        // 只有 `socket:[]` 是有效 socket（inode 0）；其余全部忽略。
        assert_eq!(decoded.owners.len(), 1);
        assert_eq!(decoded.owners[&0], vec![b"/app/one".to_vec()]);
    }

    #[test]
    fn long_executable_grows_the_reused_buffer() {
        let _guard = test_lock();
        let fixture = Fixture::new("long-exe");
        let boundary = long_path(128); // readlink 恰好填满初始 128 字节缓冲
        let large = long_path(701); // 需要连续三次倍增
        fixture.process("1", &boundary, &[("0", b"socket:[1]")]);
        fixture.process("2", &large, &[("0", b"socket:[2]")]);
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(
            decoded.executables,
            vec![boundary.clone(), large.clone()],
            "long exe bytes must be preserved exactly"
        );
        assert_eq!(decoded.owners[&1], vec![boundary]);
        assert_eq!(decoded.owners[&2], vec![large]);
    }

    #[test]
    fn readdir_failure_discards_only_partial_process() {
        let _guard = test_lock();
        let fixture = Fixture::new("inject");
        fixture.process(
            "1",
            b"/app/first",
            &[
                ("0", b"socket:[10]"),
                ("1", b"socket:[11]"),
                ("2", b"socket:[12]"),
            ],
        );
        fixture.process("2", b"/app/second", &[("0", b"socket:[20]")]);
        let reset = FailAfterReset;

        // 处理 1 个 fd 项后注入错误：进程 1 的暂存项必须全部丢弃，进程 2 正常。
        FD_READDIR_FAIL_AFTER.store(1, Ordering::SeqCst);
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(
            decoded.owners.keys().copied().collect::<Vec<u32>>(),
            vec![20],
            "partial process data must not be merged"
        );
        assert_eq!(decoded.owners[&20], vec![b"/app/second".to_vec()]);

        // 注入点在第一项之前：进程 1 完全被丢弃，不应登记任何路径。
        FD_READDIR_FAIL_AFTER.store(0, Ordering::SeqCst);
        let decoded = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(
            decoded.owners.keys().copied().collect::<Vec<u32>>(),
            vec![20]
        );
        assert_eq!(decoded.executables, vec![b"/app/second".to_vec()]);
        drop(reset);
    }

    /// 即使测试断言失败，也要把注入点复位，避免污染同一进程里的后续测试。
    struct FailAfterReset;

    impl Drop for FailAfterReset {
        fn drop(&mut self) {
            FD_READDIR_FAIL_AFTER.store(usize::MAX, Ordering::SeqCst);
        }
    }

    #[test]
    fn repeated_scans_produce_the_same_snapshot() {
        let _guard = test_lock();
        let fixture = reference_fixture();
        let first = scan_decoded(fixture.root_bytes(), ALL_USERS);
        let second = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(first, second);
        assert_eq!(first.owners, expected_owners());
    }

    /// bench-dual 构建里可以直接和 Rust1 对照：错误语义与结果必须逐字节语义一致。
    #[cfg(feature = "bench-dual")]
    #[test]
    fn rust1_and_rust2_agree_on_fixtures() {
        let _guard = test_lock();
        let fixture = reference_fixture();
        let rust1 = decode(&crate::rust1::scan(fixture.root_bytes(), ALL_USERS).unwrap());
        let rust2 = scan_decoded(fixture.root_bytes(), ALL_USERS);
        assert_eq!(rust1, rust2, "reference fixture differs from Rust1");

        let loop_fixture = Fixture::new("rust1-loop");
        let directory = loop_fixture.process_dir("10");
        fs::create_dir_all(directory.join("fd")).unwrap();
        symlink("exe", directory.join("exe")).unwrap();
        symlink("socket:[42]", directory.join("fd").join("1")).unwrap();
        let rust1 = decode(&crate::rust1::scan(loop_fixture.root_bytes(), ALL_USERS).unwrap());
        let rust2 = scan_decoded(loop_fixture.root_bytes(), ALL_USERS);
        assert_eq!(rust1, rust2, "self-referential exe differs from Rust1");

        let error_fixture = Fixture::new("rust1-error");
        let directory = error_fixture.process_dir("10");
        fs::create_dir_all(directory.join("fd")).unwrap();
        fs::write(directory.join("exe"), b"").unwrap();
        let rust1_error = crate::rust1::scan(error_fixture.root_bytes(), ALL_USERS).unwrap_err();
        let rust2_error = scan(error_fixture.root_bytes(), ALL_USERS).unwrap_err();
        assert_eq!(
            rust1_error.raw_os_error(),
            rust2_error.raw_os_error(),
            "regular exe error differs from Rust1"
        );
    }

    fn fd_count() -> usize {
        fs::read_dir("/proc/self/fd").unwrap().count()
    }

    /// 采样最小值，过滤掉其它测试线程短暂的 FD 峰值；真实泄漏不会被过滤。
    fn stable_fd_count() -> usize {
        (0..5)
            .map(|_| {
                thread::sleep(Duration::from_millis(2));
                fd_count()
            })
            .min()
            .unwrap()
    }

    #[test]
    fn scanning_does_not_leak_descriptors() {
        let _guard = test_lock();
        let fixture = Fixture::new("fds");
        fixture.process("1", b"/app/a", &[("0", b"socket:[1]")]);
        fixture.process("2", b"/app/b", &[("0", b"socket:[2]")]);
        let before = stable_fd_count();
        for _ in 0..200 {
            assert!(scan(fixture.root_bytes(), ALL_USERS).is_ok());
        }
        let after = stable_fd_count();
        assert_eq!(after, before, "scan leaked file descriptors");
    }

    #[test]
    fn rust2_variant_marker_is_exported() {
        assert_eq!(crate::magicnet_proc_scanner_rust2_marker(), 2);
        assert_eq!(crate::magicnet_proc_scanner_variant(), 2);
    }

    #[test]
    #[ignore = "requires a live /proc"]
    fn real_proc_scan_smoke() {
        let _guard = test_lock();
        let decoded = scan_decoded(b"/proc", ALL_USERS);
        assert!(!decoded.owners.is_empty(), "no sockets found in live /proc");
        assert!(
            decoded
                .owners
                .values()
                .all(|owners| owners.iter().all(|path| !path.is_empty()))
        );
        let owner = unsafe { libc::geteuid() };
        let owned = scan_decoded(b"/proc", owner);
        for (inode, paths) in &owned.owners {
            let all = decoded.owners.get(inode).expect("owned inode missing");
            for path in paths {
                assert!(all.contains(path), "uid filter changed an owner path");
            }
        }
    }
}
