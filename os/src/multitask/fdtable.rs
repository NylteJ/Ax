use core::{fmt::Debug, iter::zip};

use alloc::vec::Vec;
use bitvec::prelude::*;

use crate::filesystem::vfs::FileArc;

/// 文件描述符表，管理一个任务的所有文件描述符
///
/// 其克隆操作是浅拷贝，双方仍共享同一批 File
#[derive(Clone)]
pub struct FileDescriptorTable {
    entries: Vec<Option<FDTableEntry>>,
    free: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct FileDescriptorFlags {
    cloexec: bool,
}
impl FileDescriptorFlags {
    pub fn from_raw(raw: usize) -> Self {
        let cloexec = raw & 1 == 1;
        Self { cloexec }
    }
    pub fn into_raw(self) -> usize {
        self.cloexec.into()
    }

    pub fn is_cloexec(&self) -> bool {
        self.cloexec
    }
    pub fn set_cloexec(&mut self, cloexec: bool) {
        self.cloexec = cloexec;
    }

    pub fn empty() -> Self {
        Self { cloexec: false }
    }
}

#[derive(Clone)]
pub struct FDTableEntry(pub FileArc, pub FileDescriptorFlags);

impl FDTableEntry {
    pub fn no_flags(file: FileArc) -> Self {
        Self(file, FileDescriptorFlags::empty())
    }
    pub fn cloexec(file: FileArc) -> Self {
        Self(file, FileDescriptorFlags { cloexec: true })
    }
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn insert(&mut self, entry: FDTableEntry, rlimit: usize) -> Result<usize, InsertFDError> {
        self.insert_at_least(0, entry, rlimit)
    }

    /// 将一个不小于 `min_fd` 的文件描述符分配给 `entry`，返回实际分配的描述符
    ///
    /// 若即将分配的描述符编号将大于 `max_fd` 则分配失败返回 [`Err`]
    pub fn insert_at_least(
        &mut self,
        min_fd: usize,
        entry: FDTableEntry,
        rlimit: usize,
    ) -> Result<usize, InsertFDError> {
        if min_fd >= self.entries.len() {
            if min_fd >= rlimit {
                return Err(InsertFDError::ExceedRLimit);
            }

            self.set(min_fd, entry);
            return Ok(min_fd);
        }

        let free = &mut self.free.view_bits_mut::<LocalBits>()[min_fd..];

        if let Some(offset) = free.first_one() {
            let index = min_fd + offset;
            if index >= rlimit {
                return Err(InsertFDError::ExceedRLimit);
            }

            self.entries[index] = Some(entry);
            free.set(offset, false);
            Ok(index)
        } else {
            let index = self.entries.len();
            if index >= rlimit {
                return Err(InsertFDError::ExceedRLimit);
            }

            self.entries.push(Some(entry));
            if self.entries.len() > free.len() + min_fd {
                self.free.push(0);
            }
            Ok(index)
        }
    }

    pub fn get(&self, fd: usize) -> Option<&FDTableEntry> {
        self.entries.get(fd)?.as_ref()
    }
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FDTableEntry> {
        self.entries.get_mut(fd)?.as_mut()
    }

    /// 设置给定文件描述符的 File 指针，若原先该描述符空缺则会发生分配。若不空缺则返回原值。
    ///
    /// 最坏情况下其会浪费大量内存，小心使用！
    pub fn set(&mut self, fd: usize, entry: FDTableEntry) -> Option<FDTableEntry> {
        let old_len = self.entries.len();
        if fd >= old_len {
            self.entries.resize(fd + 1, None);
            self.free.resize((fd + 1).div_ceil(usize::BITS as usize), 0);
            self.free.view_bits_mut::<LocalBits>()[old_len..fd].fill(true);
        }

        self.entries[fd].replace(entry)
    }

    pub fn remove(&mut self, fd: usize) -> Option<FDTableEntry> {
        let free = self.free.view_bits_mut::<LocalBits>();

        let entry = self.entries.get_mut(fd)?;
        let file = entry.take()?;
        free.set(fd, true);
        Some(file)
    }

    pub fn close_on_exec(&mut self) {
        for (entry, mut free) in zip(
            self.entries.iter_mut(),
            self.free.view_bits_mut::<LocalBits>().iter_mut(),
        ) {
            if let Some(FDTableEntry(_, flags)) = entry
                && flags.is_cloexec()
            {
                *entry = None;
                *free = true;
            }
        }
    }
}

pub enum InsertFDError {
    ExceedRLimit,
}

impl Debug for FileDescriptorTable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileDescriptorTable")
            .field_with("files", |formatter| {
                let mut list = formatter.debug_list();

                for (i, entry) in self.entries.iter().enumerate() {
                    if let Some(_) = entry {
                        debug_assert!(!self.free.view_bits::<LocalBits>()[i]);
                        list.entry(&format_args!("{i}: (some file)"));
                    } else {
                        debug_assert!(self.free.view_bits::<LocalBits>()[i]);
                    }
                }

                list.finish()
            })
            .finish()
    }
}
