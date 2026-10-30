//! 紧凑持有者表：`inode → 首个路径 ID + 附加路径索引`。
//!
//! 每个 inode 的值固定为两个 `u32`（8 字节）；只有出现第二个不同路径 ID 时才创建
//! 附加列表。绝大多数 socket 只有单个持有者，因此不产生逐 inode 的 `Vec` 分配，
//! 同时保留去重、首次出现顺序与完整持有者集合。

use std::collections::HashMap;

/// 附加列表索引的哨兵值：没有附加持有者。
const NO_EXTRA: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Owner {
    first: u32,
    extra: u32,
}

pub(crate) struct OwnerTable {
    entries: HashMap<u32, Owner>,
    extras: Vec<Vec<u32>>,
}

impl OwnerTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            extras: Vec::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 记录一个持有者。已存在相同路径 ID 时不重复添加。
    pub(crate) fn insert(&mut self, inode: u32, path_id: u32) {
        match self.entries.get_mut(&inode) {
            None => {
                self.entries.insert(
                    inode,
                    Owner {
                        first: path_id,
                        extra: NO_EXTRA,
                    },
                );
            }
            Some(owner) => {
                if owner.first == path_id {
                    return;
                }
                if owner.extra == NO_EXTRA {
                    self.extras.push(vec![path_id]);
                    owner.extra = (self.extras.len() - 1) as u32;
                    return;
                }
                let list = &mut self.extras[owner.extra as usize];
                if !list.contains(&path_id) {
                    list.push(path_id);
                }
            }
        }
    }

    /// 遍历 `(inode, 首个路径 ID, 附加路径 ID 切片)`；顺序为首次出现顺序。
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, u32, &[u32])> + '_ {
        self.entries.iter().map(move |(&inode, owner)| {
            let extras = if owner.extra == NO_EXTRA {
                &[][..]
            } else {
                self.extras[owner.extra as usize].as_slice()
            };
            (inode, owner.first, extras)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(table: &OwnerTable) -> std::collections::BTreeMap<u32, Vec<u32>> {
        table
            .iter()
            .map(|(inode, first, extras)| {
                let mut owners = vec![first];
                owners.extend_from_slice(extras);
                (inode, owners)
            })
            .collect()
    }

    #[test]
    fn single_holder_has_no_extra_list() {
        let mut table = OwnerTable::new();
        table.insert(7, 3);
        table.insert(7, 3);
        assert_eq!(table.extras.len(), 0);
        assert_eq!(collect(&table), [(7, vec![3])].into());
    }

    #[test]
    fn multiple_holders_keep_first_seen_order_and_dedupe() {
        let mut table = OwnerTable::new();
        table.insert(1, 10);
        table.insert(1, 11);
        table.insert(1, 10);
        table.insert(1, 12);
        table.insert(1, 11);
        table.insert(2, 20);
        assert_eq!(table.extras.len(), 1);
        assert_eq!(table.len(), 2);
        assert_eq!(
            collect(&table),
            [(1, vec![10, 11, 12]), (2, vec![20])].into()
        );
    }

    #[test]
    fn many_inodes_share_one_extra_vector_per_multi_holder() {
        let mut table = OwnerTable::new();
        for inode in 0..256 {
            table.insert(inode, inode);
        }
        for inode in 0..8 {
            table.insert(inode, inode + 1000);
        }
        assert_eq!(table.extras.len(), 8);
        assert_eq!(table.len(), 256);
    }
}
