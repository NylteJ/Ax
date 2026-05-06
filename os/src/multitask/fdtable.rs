use core::{fmt::Debug, pin::Pin};

use alloc::{sync::Arc, vec::Vec};
use bitvec::prelude::*;

use crate::filesystem::vfs::File;

pub struct FileDescriptorTable {
    entries: Vec<Option<Pin<Arc<File>>>>,
    free: Vec<usize>,
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free: Vec::new(),
        }
    }

    pub fn insert(&mut self, file: Pin<Arc<File>>) -> usize {
        let free = self.free.view_bits_mut::<LocalBits>();

        if let Some(index) = free.first_one() {
            self.entries[index] = Some(file);
            free.set(index, false);
            index
        } else {
            let index = self.entries.len();
            self.entries.push(Some(file));
            if self.entries.len() > free.len() {
                self.free.push(0);
            }
            index
        }
    }

    pub fn get(&self, fd: usize) -> Option<&Pin<Arc<File>>> {
        self.entries.get(fd)?.as_ref()
    }
    pub fn get_mut(&mut self, fd: usize) -> Option<&mut Pin<Arc<File>>> {
        self.entries.get_mut(fd)?.as_mut()
    }

    /// 设置给定文件描述符的 File 指针，若原先该描述符空缺则会发生分配。若不空缺则返回原值。
    ///
    /// 最坏情况下其会浪费大量内存，小心使用！
    pub fn set(&mut self, fd: usize, file: Pin<Arc<File>>) -> Option<Pin<Arc<File>>> {
        let old_len = self.entries.len();
        if fd >= old_len {
            self.entries.resize(fd + 1, None);
            self.free.resize((fd + 1).div_ceil(usize::BITS as usize), 0);
            self.free.view_bits_mut::<LocalBits>()[old_len..fd].fill(true);
        }

        self.entries[fd].replace(file)
    }

    pub fn remove(&mut self, fd: usize) -> Option<Pin<Arc<File>>> {
        let free = self.free.view_bits_mut::<LocalBits>();

        let entry = self.entries.get_mut(fd)?;
        let file = entry.take()?;
        free.set(fd, true);
        Some(file)
    }
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
