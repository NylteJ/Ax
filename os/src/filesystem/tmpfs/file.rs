use core::any::Any;

use crate::{
    filesystem::{
        tmpfs::{INodeType, TmpFsINode},
        utils::{FileIoPgCacheMissResult, file_io_common},
        vfs::{self, FileFlags, IterateDirData, VfsError, VfsResult, default_fseek},
    },
    memory::uaccess::{copy_from_user_uninit, copy_to_user},
};

pub struct TmpFsFile {}

impl vfs::FileOps for TmpFsFile {
    fn read(&mut self, mut user_buf: *mut u8, len: u64) -> VfsResult<u64> {
        let this = vfs::File::<Self>::from_dyn_mut(self);
        let inode_read_guard = unsafe { this.inode.as_ref() }.data.read();
        let pos = &mut this.data.get_mut().pos;
        file_io_common(
            false,
            pos,
            inode_read_guard.page_cache.as_ref().unwrap(),
            len.min(inode_read_guard.file_size - *pos),
            |buf| {
                let k_src = unsafe { buf.assume_init_ref() };
                copy_to_user(user_buf, k_src)?;
                user_buf = unsafe { user_buf.add(buf.len()) };
                Ok(())
            },
            |_, _| Ok(FileIoPgCacheMissResult::Break),
        )
    }

    fn write(&mut self, mut user_data: *const u8, len: u64) -> VfsResult<u64> {
        let this = vfs::File::<Self>::from_dyn_mut(self);
        let mut inode_write_guard = unsafe { this.inode.as_ref() }.data.write();

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
            |buf| {
                copy_from_user_uninit(buf, user_data)?;
                user_data = unsafe { user_data.add(buf.len()) };
                Ok(())
            },
            |page_cache, page_index| {
                page_cache.alloc(page_index);
                Ok(FileIoPgCacheMissResult::Retry)
            },
        )?;

        inode_write_guard.file_size = inode_write_guard.file_size.max(old_pos + len);
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
        debug_assert!((&unsafe { inode.as_ref() }.data.read().ops as &dyn Any).is::<TmpFsINode>());
        let inode = unsafe { inode.cast::<vfs::INode<TmpFsINode>>().as_ref() };

        let inode_data_guard = inode.data.read();
        let INodeType::Directory(dir_vec) = &inode_data_guard.ops.inode_type else {
            return Err(VfsError::NotADirectory);
        };

        Ok(dir_vec.get(pos as usize).map(|(name, _)| IterateDirData {
            name: name.clone(),
            next_pos: pos as i64 + 1,
            inode_id: inode.id,
        }))
    }
}
