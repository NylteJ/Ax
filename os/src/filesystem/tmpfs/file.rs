use core::{any::Any, mem::MaybeUninit};

use uninit::extension_traits::MaybeUninitExt;

use crate::filesystem::{
    tmpfs::{INodeType, TmpFsINode},
    utils::{FileIoPgCacheMissResult, file_io_common},
    vfs::{self, FileFlags, IterateDirData, VfsError, VfsResult, default_fseek},
};

pub struct TmpFsFile {}

impl vfs::FileOps for TmpFsFile {
    fn kread(
        &mut self,
        visitor: &mut dyn FnMut(&[u8]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let this = vfs::File::<Self>::from_dyn_mut(self);
        let inode_read_guard = unsafe { this.inode.as_ref() }.data.read();
        let pos = &mut this.data.get_mut().pos;

        if inode_read_guard.file_size <= *pos {
            return Ok(0);
        }

        file_io_common(
            false,
            pos,
            inode_read_guard.page_cache.as_ref().unwrap(),
            len.min(inode_read_guard.file_size - *pos),
            |buf| visitor(unsafe { buf.assume_init_by_ref() }),
            |_, _| Ok(FileIoPgCacheMissResult::Break),
        )
    }

    fn kwrite(
        &mut self,
        visitor: &mut dyn FnMut(&mut [MaybeUninit<u8>]) -> VfsResult<u64>,
        len: u64,
    ) -> VfsResult<u64> {
        let this = vfs::File::<Self>::from_dyn_mut(self);
        debug_assert!((&this.get_inode().data.read().ops as &dyn Any).is::<TmpFsINode>());
        let inode = unsafe { this.inode.cast::<vfs::INode<TmpFsINode>>().as_ref() };

        let mut inode_write_guard = inode.data.write();

        let this_data_mut = this.data.get_mut();
        let pos = &mut this_data_mut.pos;

        if this_data_mut.flags.contains(FileFlags::Append) {
            *pos = inode_write_guard.file_size;
        }

        let old_pos = *pos;

        let ret = file_io_common(
            true,
            pos,
            inode_write_guard.page_cache.as_ref().unwrap(),
            len,
            visitor,
            |page_cache, page_index| {
                page_cache.alloc(page_index);
                Ok(FileIoPgCacheMissResult::Retry)
            },
        )?;

        let new_file_size = inode_write_guard.file_size.max(old_pos + len);
        inode_write_guard.file_size = new_file_size;
        inode_write_guard.ops.stat.st_size = new_file_size;
        Ok(ret)
    }

    fn fseek(&mut self, seek_from: vfs::SeekFrom) -> VfsResult<u64> {
        default_fseek(self, seek_from)
    }

    fn fsync(&mut self) -> VfsResult<()> {
        Ok(()) // tmpfs 没什么能同步的
    }

    fn get_dir(&self, pos: u64) -> VfsResult<Option<IterateDirData>> {
        let this = vfs::File::from_dyn(self);

        let inode = this.inode;
        debug_assert!((&this.get_inode().data.read().ops as &dyn Any).is::<TmpFsINode>());
        let inode = unsafe { inode.cast::<vfs::INode<TmpFsINode>>().as_ref() };

        let inode_data_guard = inode.data.read();
        let INodeType::Directory(dir_vec) = &inode_data_guard.ops.inode_type else {
            return Err(VfsError::NotADirectory);
        };

        Ok(dir_vec.get(pos as usize).map(|(name, child_inode)| {
            let child = unsafe { child_inode.downcast_ref::<TmpFsINode>().unwrap_unchecked() };
            let child_guard = child.data.read();
            IterateDirData {
                name: name.clone(),
                next_pos: pos + 1,
                inode_id: child.id,
                entry_type: match child_guard.ops.inode_type {
                    INodeType::File => vfs::IterateDirEntryType::RegularFile,
                    INodeType::Directory(..) => vfs::IterateDirEntryType::Directory,
                    INodeType::Symlink(..) => vfs::IterateDirEntryType::Symlink,
                },
            }
        }))
    }

    fn poll(&self) -> VfsResult<vfs::PollStatus> {
        Ok(vfs::PollStatus::regular_io_ready())
    }
}
