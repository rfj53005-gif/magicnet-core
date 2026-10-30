//! 两种扫描实现共用的纯函数：PID 名称判定、socket inode 解析、可忽略错误的判定。

use std::io;

use crate::unicode_digits;

/// 目录项名称是否为 PID：ASCII 数字，或 Go `unicode.IsDigit` 覆盖的任何十进制数字。
pub(crate) fn pid_name(name: &[u8]) -> bool {
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

/// 解析 `socket:[inode]`；保留 CPU1/Rust1 的空 body 行为（得到 0）与溢出拒绝。
pub(crate) fn socket_inode(link: &[u8]) -> Option<u32> {
    let body = link.strip_prefix(b"socket:[")?.strip_suffix(b"]")?;
    let mut inode = 0u32;
    for &ch in body {
        if !ch.is_ascii_digit() {
            return None;
        }
        inode = inode.checked_mul(10)?.checked_add((ch - b'0') as u32)?;
    }
    Some(inode)
}

/// 进程在扫描期间消失或无权访问时跳过，其余错误向调用方报告。
pub(crate) fn ignorable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    )
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
}
