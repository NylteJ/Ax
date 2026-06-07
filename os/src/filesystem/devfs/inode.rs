use core::{ptr::NonNull, sync::atomic::Ordering};

use alloc::{borrow::ToOwned, vec::Vec};

use crate::{
    basic::{
        bstr::{ByteStr, ByteString},
        extend_lifetime,
    },
    filesystem::{
        devfs::{
            dentry::DevDEntry,
            drivers::{DeviceId, DeviceType},
            filesystem::DevFs,
        },
        vfs::{self, INodeOps, VfsError, VfsResult},
    },
    syscall::filesystem::KstatInner,
};

pub(super) enum DevINode {
    Directory(Vec<(ByteString, vfs::INodeArc)>),
    DeviceFile(DeviceType, DeviceId),
    Symlink(!), // FIXME
}

impl INodeOps for DevINode {
    fn create(
        &mut self,
        _mode: vfs::INodeMode,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::OperationNotSupported)
    }

    fn lookup(&self, name: &ByteStr, dentry: vfs::DEntryArc) -> VfsResult<vfs::DEntryArc> {
        let DevINode::Directory(children) = self else {
            return Err(VfsError::NotADirectory);
        };

        Ok(vfs::DEntry::lock_and_new(
            dentry,
            name.to_owned(),
            children
                .binary_search_by_key(&name, |(n, _)| n.as_ref())
                .ok()
                .map(|pos| children[pos].1.clone()),
            vfs::INode::get_filesystem::<DevFs>(self),
            DevDEntry,
        ))
    }

    fn link(&mut self, _dentry: vfs::DEntryArc, _target: &vfs::INodeArc) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn unlink(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        // 注意 unlink 还是要实现的
        let DevINode::Directory(children) = self else {
            return Err(VfsError::NotADirectory);
        };

        let name = dentry.data.lock().name.clone();
        let pos = children
            .binary_search_by_key(&&name, |(n, _)| n)
            .map_err(|_| VfsError::FileNotFound)?;

        children.remove(pos);
        Ok(())
    }

    fn mkdir(&mut self, dentry: vfs::DEntryArc, mode: vfs::INodeMode) -> VfsResult<vfs::INodeArc> {
        let fs = vfs::INode::get_filesystem::<DevFs>(self);
        let fs = unsafe { extend_lifetime(fs) };

        let Self::Directory(children) = self else {
            return Err(VfsError::NotADirectory);
        };

        let name = dentry.data.lock().name.clone();
        let Err(pos) = children.binary_search_by_key(&&name, |(n, _)| n) else {
            return Err(VfsError::FileAlreadyExists);
        };

        let id = fs.ops.inode_id.fetch_add(1, Ordering::Relaxed);

        let new_inode = vfs::INode::new(id, NonNull::from_ref(fs), vfs::INodeData {
            file_size: 0,
            hard_links: 2,
            mode,
            page_cache: None,
            ops: DevINode::Directory(Vec::new()),
        });

        children.insert(pos, (name, new_inode.clone()));

        Ok(new_inode)
    }

    fn rmdir(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn rename(
        &mut self,
        _new_node: Option<&mut vfs::INodeData>,
        _old_name: &ByteStr,
        _new_name: ByteString,
        _flags: vfs::INodeRenameFlags,
    ) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn make_node(
        &mut self,
        kind: vfs::SpecialINodeType,
        mode: vfs::INodeMode,
        dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        let fs = vfs::INode::get_filesystem::<DevFs>(self);
        let fs = unsafe { extend_lifetime(fs) };

        let DevINode::Directory(children) = self else {
            return Err(VfsError::NotADirectory);
        };

        let (device_type, device_id) = match kind {
            vfs::SpecialINodeType::BlockDevice(id) => (DeviceType::Block, id),
            vfs::SpecialINodeType::CharDevice(id) => (DeviceType::Char, id),
            _ => return Err(VfsError::OperationNotSupported),
        };

        let name = dentry.data.lock().name.clone();
        let pos = children
            .binary_search_by_key(&&name, |(n, _)| n)
            .expect_err("VFS 层不应传入已经证实存在的名字作为 make_node 的目标（devfs 相当内部，因此 panic 是可接受的）");

        let id = fs.ops.inode_id.fetch_add(1, Ordering::Relaxed);

        let new_inode = vfs::INode::new(id, NonNull::from_ref(fs), vfs::INodeData {
            file_size: 0,  // 设备文件的大小没什么意义
            hard_links: 1, // 我们不维护它
            mode,
            page_cache: None, // 无需页缓存（块设备也是使用底层缓存）
            ops: DevINode::DeviceFile(device_type, device_id),
        });

        children.insert(pos, (name, new_inode.clone()));

        Ok(new_inode)
    }

    fn get_kstat(&self) -> VfsResult<KstatInner> {
        match self {
            DevINode::Directory(..) => Ok(Default::default()), // FIXME
            DevINode::DeviceFile(_, device_id) => Ok(KstatInner {
                st_rdev: device_id.to_u64(),
                ..Default::default() // FIXME?
            }),
            DevINode::Symlink(x) => *x,
        }
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        true // 不支持硬链接，因此也不必查询了（我们甚至不维护硬链接计数）
    }
}
