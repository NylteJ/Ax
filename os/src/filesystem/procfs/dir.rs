use core::{
    any::Any, mem::MaybeUninit, ops::Deref, pin::Pin, ptr::NonNull, sync::atomic::Ordering,
};

use alloc::{borrow::ToOwned, sync::Arc, vec::Vec};

use crate::{
    basic::bstr::{ByteStr, ByteString},
    filesystem::{
        devfs::drivers::DeviceId,
        procfs::{dentry::ProcFsDentry, filesystem::ProcFs, inode::ProcFsINode},
        vfs::{self, DEntryOps, FileOps, INodeOps, VfsError, VfsResult, default_fseek},
    },
    syscall::filesystem::{KstatInner, Time},
};

pub struct ProcFsDirDEntry;

impl DEntryOps for ProcFsDirDEntry {
    fn open(
        &self,
        mount: Arc<vfs::Mount>,
        flags: vfs::FileFlags,
    ) -> VfsResult<Pin<Arc<vfs::File>>> {
        let inode = vfs::DEntryData::get_data(self)
            .inode
            .as_ref()
            .expect("VFS 层永远不应在负 DEntry 上调用 open")
            .deref();

        let dentry: &vfs::DEntry = vfs::DEntry::<Self>::get_dentry(self);
        let dentry = unsafe { vfs::get_varc_from_value(dentry) };

        Ok(vfs::File::new(
            vfs::Path(dentry, mount),
            inode,
            flags,
            ProcFsDirFile,
        ))
    }
}

pub struct ProcFsDirINode {
    pub data: Vec<(ByteString, vfs::INodeArc)>,
}

impl INodeOps for ProcFsDirINode {
    fn create(
        &mut self,
        _mode: vfs::INodeMode,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::OperationNotSupported)
    }

    fn lookup(&self, name: &ByteStr, dentry: vfs::DEntryArc) -> VfsResult<vfs::DEntryArc> {
        let children = &self.data;

        Ok(vfs::DEntry::lock_and_new(
            dentry,
            name.to_owned(),
            children
                .binary_search_by_key(&name, |(n, _)| n.as_ref())
                .ok()
                .map(|pos| children[pos].1.clone()),
            vfs::INode::get_filesystem::<ProcFs>(self),
            ProcFsDentry,
        ))
    }

    fn link(&mut self, _dentry: vfs::DEntryArc, _target: &vfs::INodeArc) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn unlink(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn mkdir(
        &mut self,
        _dentry: vfs::DEntryArc,
        _mode: vfs::INodeMode,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::OperationNotSupported)
    }

    fn rmdir(&mut self, _dentry: &vfs::DEntryArc) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn rename(
        &mut self,
        _new_node: Option<&mut vfs::INodeData>,
        _old_name: &crate::basic::bstr::ByteStr,
        _new_name: ByteString,
        _flags: vfs::INodeRenameFlags,
    ) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }

    fn symlink(
        &mut self,
        _target: ByteString,
        _dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        Err(VfsError::OperationNotSupported)
    }

    fn read_symlink(&self) -> VfsResult<ByteString> {
        Err(VfsError::IsADirectory)
    }

    fn get_kstat(&self) -> VfsResult<KstatInner> {
        Ok(KstatInner {
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            st_size: 0,
            st_blksize: 0,
            st_blocks: 0,
            st_atime: Time::from_ns(0), // FIXME?
            st_mtime: Time::from_ns(0), // FIXME?
            st_ctime: Time::from_ns(0), // FIXME?
            ..Default::default()
        })
    }

    fn make_node(
        &mut self,
        kind: vfs::SpecialINodeType,
        mode: vfs::INodeMode,
        dentry: vfs::DEntryArc,
    ) -> VfsResult<vfs::INodeArc> {
        // 我们复用这个接口

        let name = dentry.data.lock().name.clone();

        let fs = vfs::INode::get_filesystem::<ProcFs>(self);
        let fs = NonNull::from_ref(fs);

        let map = &mut self.data;

        let Err(pos) = map.binary_search_by_key(&&name, |(n, _)| n) else {
            debug_assert!(false);
            return Err(VfsError::FileAlreadyExists);
        };

        fn get_node<T: INodeOps>(fs: NonNull<vfs::FileSystem<ProcFs>>, ops: T) -> vfs::INodeArc {
            vfs::INode::new(
                unsafe { fs.as_ref() }
                    .ops
                    .counter
                    .fetch_add(1, Ordering::Relaxed),
                fs,
                vfs::INodeData {
                    file_size: 0,
                    hard_links: 1,
                    mode: vfs::INodeMode::all_permitted() | vfs::INodeMode::RegularFile,
                    page_cache: None,
                    ops,
                },
            )
        }

        let new_inode = match kind {
            vfs::SpecialINodeType::BlockDevice(DeviceId(999, 0)) => {
                get_node(fs, ProcFsINode::MemInfo)
            }
            _ => return Err(VfsError::OperationNotSupported),
        };

        map.insert(pos, (name, new_inode.clone()));

        Ok(new_inode)
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        false
    }
}

pub struct ProcFsDirFile;

impl FileOps for ProcFsDirFile {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::IsADirectory)
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        Err(VfsError::IsADirectory)
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        default_fseek(self, seek_from)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Ok(())
    }

    fn get_dir(&self, pos: u64) -> VfsResult<Option<vfs::IterateDirData>> {
        let this = vfs::File::from_dyn(self);

        let inode = this.inode;
        debug_assert!((&this.get_inode().data.read().ops as &dyn Any).is::<ProcFsDirINode>());
        let inode = unsafe { inode.cast::<vfs::INode<ProcFsDirINode>>().as_ref() };

        Ok(inode
            .data
            .read()
            .ops
            .data
            .get(pos as usize)
            .map(|(name, _)| vfs::IterateDirData {
                name: name.clone(),
                next_pos: pos + 1,
                inode_id: inode.id,
                entry_type: match inode.data.read().mode.kind() {
                    vfs::INodeMode::RegularFile => vfs::IterateDirEntryType::RegularFile,
                    vfs::INodeMode::Directory => vfs::IterateDirEntryType::Directory,
                    vfs::INodeMode::SymbolLink => vfs::IterateDirEntryType::Symlink,
                    _ => unreachable!(),
                },
            }))
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        todo!()
    }
}
