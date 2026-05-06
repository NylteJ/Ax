use core::bstr::ByteStr;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use alloc::borrow::ToOwned;
use alloc::bstr::ByteString;
use alloc::vec::Vec;

use crate::filesystem::tmpfs::{TmpFs, TmpFsDentry};
use crate::filesystem::vfs::{
    INodeArc, INodeMode, INodeOps, INodeRenameFlags, VfsError, VfsResult,
    default_should_drop_when_mem_refcount_zero,
};

use crate::filesystem::vfs;

pub(super) enum INodeType {
    File,
    Directory(Vec<(ByteString, INodeArc)>),
    Symlink(ByteString),
}

pub struct TmpFsINode {
    pub inode_type: INodeType,
}

impl INodeOps for TmpFsINode {
    fn create(&mut self, mode: INodeMode, dentry: vfs::DEntryArc) -> VfsResult<vfs::INodeArc> {
        let name = dentry.data.lock().name.clone();

        let fs = vfs::INode::get_filesystem::<TmpFs>(self);
        let fs = NonNull::from_ref(fs);

        let map = self.as_directory_mut()?;

        let Err(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            debug_assert!(false);
            return Err(VfsError::FileAlreadyExists);
        };

        let inode = map
            .insert_mut(
                pos,
                (
                    name,
                    Self::new_normal(unsafe { fs.as_ref() }, INodeType::File, mode),
                ),
            )
            .1
            .clone();

        let old_inode = dentry.data.lock().inode.replace(inode.clone());
        debug_assert!(old_inode.is_none());

        Ok(inode)
    }

    fn lookup(&self, name: &ByteStr, dentry: vfs::DEntryArc) -> VfsResult<vfs::DEntryArc> {
        let INodeType::Directory(vec) = &self.inode_type else {
            return Err(VfsError::NotADirectory);
        };

        Ok(vfs::DEntry::lock_and_new(
            dentry,
            name.to_owned(),
            vec.binary_search_by_key(&name, |(n, _)| n.as_ref())
                .ok()
                .map(|pos| vec[pos].1.clone()),
            vfs::INode::get_filesystem::<TmpFs>(self),
            TmpFsDentry {},
        ))
    }

    fn link(&mut self, dentry: vfs::DEntryArc, target: &vfs::INodeArc) -> VfsResult<()> {
        let target = target.clone().downcast_or_vfserr::<TmpFsINode>()?;
        if let INodeType::Directory(_) = target.data.read().dynamic_data.inode_type {
            return Err(VfsError::IsADirectory);
        }

        let map = self.as_directory_mut()?;

        let name = dentry.data.lock().name.clone();
        let Err(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            return Err(VfsError::FileAlreadyExists);
        };

        target.data.write().hard_links += 1;
        map.insert(pos, (name, target.clone()));
        dentry.data.lock().inode.replace(target);
        Ok(())
    }

    fn unlink(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        let map = self.as_directory_mut()?;

        let name = dentry.data.lock().name.clone();
        let Ok(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            return Err(VfsError::FileNotFound);
        };

        let arc = map[pos].1.clone().downcast_or_vfserr::<TmpFsINode>()?;

        let mut data = arc.data.write();

        if let INodeType::Directory(_) = data.dynamic_data.inode_type {
            return Err(VfsError::IsADirectory);
        }

        map.remove(pos);
        data.hard_links -= 1;
        dentry.data.lock().inode.take();
        Ok(())
        // arc 在此处才销毁，若此时引用计数归零则硬链接数能正常获取
    }

    fn mkdir(&mut self, dentry: vfs::DEntryArc, mode: INodeMode) -> VfsResult<vfs::INodeArc> {
        let fs = vfs::INode::get_filesystem::<TmpFs>(self);
        let fs = NonNull::from_ref(fs);
        let map = self.as_directory_mut()?;

        let name = dentry.data.lock().name.clone();
        let Err(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            return Err(VfsError::FileAlreadyExists);
        };

        let ret = map
            .insert_mut(
                pos,
                (
                    name,
                    Self::new_normal(
                        unsafe { fs.as_ref() },
                        INodeType::Directory(Default::default()),
                        mode,
                    ),
                ),
            )
            .1
            .clone();
        vfs::INodeData::get_data_mut(self).hard_links += 1; // 子目录的 `..`
        dentry.data.lock().inode.replace(ret.clone());
        Ok(ret)
    }

    fn rmdir(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        let map = self.as_directory_mut()?;

        let name = dentry.data.lock().name.clone();
        let Ok(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            return Err(VfsError::FileNotFound);
        };

        let node = map[pos].1.downcast_ref_or_vfserr::<TmpFsINode>()?;

        let mut data = node.data.write();

        let INodeType::Directory(children) = &data.dynamic_data.inode_type else {
            return Err(VfsError::NotADirectory);
        };

        if !children.is_empty() {
            return Err(VfsError::DirectoryNotEmpty);
        }

        debug_assert_eq!(data.hard_links, 2);
        data.hard_links = 0;
        drop(data);

        map.remove(pos);

        vfs::INodeData::get_data_mut(self).hard_links -= 1; // 子目录的 `..`
        dentry.data.lock().inode.take();
        Ok(())
    }

    fn rename(
        &mut self,
        new_node: Option<&mut vfs::INodeData>,
        old_name: &ByteStr,
        new_name: ByteString,
        flags: INodeRenameFlags,
    ) -> VfsResult<()> {
        todo!()
        // fn check_compatible_and_get_nlink_diff(
        //     old: Option<INodeArc<TmpFsINode>>,
        //     new: Option<INodeArc<TmpFsINode>>,
        //     flags: INodeRenameFlags,
        // ) -> VfsResult<(i8, i8)> {
        //     let exchange = flags.contains(INodeRenameFlags::Exchange);
        //     let no_replace = flags.contains(INodeRenameFlags::NoReplace);

        //     if exchange && no_replace {
        //         return Err(VfsError::InvalidArguments(
        //             "Exchange 和 NoReplace 标志不能共存",
        //         ));
        //     }

        //     let old = old.as_deref().map(|node| node.data.read());
        //     let new = new.as_deref().map(|node| node.data.read());

        //     let old = old.as_deref();
        //     let new = new.as_deref();

        //     fn is_directory(node: &vfs::INodeData<TmpFsINode>) -> bool {
        //         matches!(node.dynamic_data.inode_type, INodeType::Directory(_))
        //     }

        //     if exchange {
        //         if let (Some(old), Some(new)) = (old, new) {
        //             if is_directory(old) != is_directory(new) {
        //                 return Err(VfsError::InvalidArguments("不能交换文件和目录"));
        //             }

        //             Ok((0, 0)) // 交换无须修改链接数
        //         } else {
        //             Err(VfsError::FileNotFound)
        //         }
        //     } else {
        //         match (old, new) {
        //             (None, _) => Err(VfsError::FileNotFound),
        //             (_, Some(_)) if no_replace => Err(VfsError::FileAlreadyExists),
        //             (Some(old), None) => {
        //                 if is_directory(old) {
        //                     Ok((-1, 1))
        //                 } else {
        //                     Ok((0, 0))
        //                 }
        //             }
        //             (Some(old), Some(new)) => {
        //                 let old_is_dir = is_directory(old);
        //                 let new_is_dir = is_directory(new);
        //                 if old_is_dir != new_is_dir {
        //                     return Err(VfsError::InvalidArguments(
        //                         "不能将文件重命名为目录或将目录重命名为文件",
        //                     ));
        //                 }

        //                 if let INodeType::Directory(map) = &new.dynamic_data.inode_type
        //                     && !map.is_empty()
        //                 {
        //                     return Err(VfsError::DirectoryNotEmpty);
        //                 }

        //                 if old_is_dir { Ok((-1, 0)) } else { Ok((0, 0)) }
        //             }
        //         }
        //     }
        // }

        // macro_rules! cast_entry {
        //     ($entry:expr) => {
        //         match &$entry {
        //             btree_map::Entry::Occupied(occupied) => {
        //                 Some(occupied.get().clone().downcast_or_vfserr::<TmpFsINode>()?)
        //             }
        //             btree_map::Entry::Vacant(_) => None,
        //         }
        //     };
        // }

        // fn dec_nlink_of_replaced(replaced: &mut INodeArc) {
        //     let replaced = unsafe { replaced.downcast_ref::<TmpFsINode>().unwrap_unchecked() };
        //     let mut data = replaced.data.write();
        //     if matches!(data.dynamic_data.inode_type, INodeType::Directory(_)) {
        //         debug_assert_eq!(data.hard_links, 2);
        //         data.hard_links = 0;
        //     } else {
        //         data.hard_links -= 1;
        //     }
        // }

        // let my_map = self.as_directory_mut()?;

        // let is_exchange = flags.contains(INodeRenameFlags::Exchange);

        // let my_diff;
        // let new_diff;

        // let new_node_data;

        // if let Some(new_node) = new_node {
        //     // 由于不用顾虑 Rust 的别名规则，跨目录的路径反而好写一些，entry 会为我们管理好状态
        //     let new_node = (&mut new_node.dynamic_data as &mut dyn Any)
        //         .downcast_mut::<TmpFsINode>()
        //         .ok_or(VfsError::FilesystemMismatch)?;
        //     let new_map = new_node.as_directory_mut()?;

        //     let old_entry = my_map.entry(old_name.to_owned());
        //     let new_entry = new_map.entry(new_name);

        //     (my_diff, new_diff) = check_compatible_and_get_nlink_diff(
        //         cast_entry!(old_entry),
        //         cast_entry!(new_entry),
        //         flags,
        //     )?;

        //     let btree_map::Entry::Occupied(mut old_entry) = old_entry else {
        //         unsafe { unreachable_unchecked() }
        //     };

        //     if is_exchange {
        //         let btree_map::Entry::Occupied(mut new_entry) = new_entry else {
        //             unsafe { unreachable_unchecked() }
        //         };

        //         swap(old_entry.get_mut(), new_entry.get_mut());
        //     } else {
        //         new_entry
        //             .and_modify(dec_nlink_of_replaced)
        //             .insert_entry(old_entry.remove());
        //     }

        //     new_node_data = vfs::INodeData::get_data_mut(new_node);
        // } else {
        //     let (old_name, old_value) = my_map
        //         .remove_entry(old_name)
        //         .ok_or(VfsError::FileNotFound)?;

        //     let old_value = match old_value.downcast::<TmpFsINode>() {
        //         Ok(inode) => inode,
        //         Err(old_value) => {
        //             my_map.insert(old_name, old_value); // 恢复原状态
        //             return Err(VfsError::FilesystemMismatch);
        //         }
        //     };

        //     let new_entry = my_map.entry(new_name);

        //     match check_compatible_and_get_nlink_diff(
        //         Some(old_value.clone()),
        //         cast_entry!(new_entry), // FIXME: 这里会破坏事务性
        //         flags,
        //     ) {
        //         Ok(diffs) => (my_diff, new_diff) = diffs,
        //         Err(err) => {
        //             my_map.insert(old_name, old_value); // 恢复原状态
        //             return Err(err);
        //         }
        //     }

        //     if is_exchange {
        //         let btree_map::Entry::Occupied(mut new_entry) = new_entry else {
        //             unsafe { unreachable_unchecked() }
        //         };
        //         let mut old_value: INodeArc = old_value;
        //         swap(new_entry.get_mut(), &mut old_value);
        //         my_map.insert(old_name, old_value); // 现在 old_value 其实是 new_value
        //     } else {
        //         new_entry
        //             .and_modify(dec_nlink_of_replaced)
        //             .insert_entry(old_value);
        //     }

        //     new_node_data = vfs::INodeData::get_data_mut(self);
        // }

        // new_node_data.hard_links = new_node_data
        //     .hard_links
        //     .wrapping_add_signed(new_diff as i32);

        // let my_data = vfs::INodeData::get_data_mut(self);
        // my_data.hard_links = my_data.hard_links.wrapping_add_signed(my_diff as i32);

        // Ok(())
    }

    fn kernel_read(&self, _offset: u64, _buf: &mut [u8]) -> VfsResult<usize> {
        panic!("tmpfs 的 kernel_read 函数不应被调用，其操作理应只停留在页缓存层级")
    }

    fn kernel_write(&mut self, _offset: u64, _buf: &[u8]) -> VfsResult<usize> {
        panic!("tmpfs 的 kernel_write 函数不应被调用，其操作理应只停留在页缓存层级")
    }

    fn should_drop_when_mem_refcount_zero(&mut self) -> bool {
        default_should_drop_when_mem_refcount_zero(self)
    }
}

impl TmpFsINode {
    pub fn new_root(fs: NonNull<vfs::FileSystem>) -> vfs::INodeArc {
        let mut ret = vfs::INode::new(
            0,
            fs,
            vfs::INodeData {
                file_size: 0,
                hard_links: 2,
                mode: INodeMode::from_bits(0o777).unwrap(),
                page_cache: None,
                dynamic_data: TmpFsINode {
                    inode_type: INodeType::Directory(Default::default()),
                },
            },
        );

        unsafe { ret.get_mut() }.data.get_mut().page_cache = Some(vfs::PageCache::new(&ret));

        ret
    }

    fn new_normal(
        fs: &vfs::FileSystem<TmpFs>,
        inode_type: INodeType,
        mode: INodeMode,
    ) -> vfs::INodeArc {
        let mut ret = vfs::INode::new(
            fs.dynamic_data.inode_id.fetch_add(1, Ordering::Relaxed),
            NonNull::from_ref(fs),
            vfs::INodeData {
                file_size: 0,
                hard_links: if let INodeType::Directory(_) = &inode_type {
                    2
                } else {
                    1
                },
                mode,
                page_cache: None,
                dynamic_data: TmpFsINode { inode_type },
            },
        );

        unsafe { ret.get_mut() }.data.get_mut().page_cache = Some(vfs::PageCache::new(&ret));

        ret
    }

    fn as_directory_mut(&mut self) -> VfsResult<&mut Vec<(ByteString, INodeArc)>> {
        match &mut self.inode_type {
            INodeType::Directory(vec) => Ok(vec),
            _ => Err(VfsError::NotADirectory),
        }
    }
}
