use core::ptr::NonNull;

use alloc::borrow::ToOwned;
use ext4_rs::ext4_defs::ROOT_INODE;

use crate::{
    basic::{
        bstr::{ByteStr, ByteString},
        extend_lifetime,
    },
    filesystem::{
        ext4::{dentry::Ext4DEntry, filesystem::Ext4Fs},
        vfs::{self, INodeOps, PageCache, VfsError, VfsResult},
    },
    syscall::filesystem::{KstatInner, Time},
};

pub struct Ext4INode {
    pub inode: ext4_rs::ext4_defs::Ext4InodeRef,
}

impl INodeOps for Ext4INode {
    fn create(&mut self, mode: vfs::INodeMode, dentry: vfs::DEntryArc) -> VfsResult<vfs::INodeArc> {
        let result = Self::new_normal(
            vfs::INode::get_filesystem(self),
            vfs::INode::get_id(self),
            dentry.data.lock().name.as_ref(),
            mode,
        );
        self.reload_from_disk();
        result
    }

    fn lookup(&self, name: &ByteStr, dentry: vfs::DEntryArc) -> VfsResult<vfs::DEntryArc> {
        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;

        let mut result = ext4_rs::ext4_defs::Ext4DirSearchResult::new(
            ext4_rs::ext4_defs::Ext4DirEntry::default(),
        );

        let inode = match ext4.dir_find_entry(vfs::INode::get_id(self), name, &mut result) {
            Ok(_) => {
                let id = result.dentry.inode;
                let cache = (fs as &vfs::FileSystem).find_inode(id);
                let inode = match cache {
                    Some(cached) => {
                        log::trace!("ext4 inode {id} cache hit");
                        cached
                    }
                    None => Self::load_normal(fs, result.dentry.inode),
                };
                Some(inode)
            }
            Err(err) if err.error() == ext4_rs::Errno::ENOENT => None,
            Err(err) => return Err(VfsError::from(err)),
        };

        Ok(vfs::DEntry::lock_and_new(
            dentry,
            name.to_owned(),
            inode,
            fs,
            Ext4DEntry,
        ))
    }

    fn link(&mut self, dentry: vfs::DEntryArc, target: &vfs::INodeArc) -> VfsResult<()> {
        let mut target_wguard = target.downcast_ref_or_vfserr::<Ext4INode>()?.data.write();
        let target_data = &mut *target_wguard;

        let fs = unsafe { extend_lifetime(vfs::INode::get_filesystem::<Ext4Fs>(self)) };
        let ext4 = &fs.ops.ext4;

        let my_inode_ref = &mut self.inode;
        let target_inode_ref = &mut target_data.ops.inode;

        ext4.link(
            my_inode_ref,
            target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        dentry.data.lock().inode.replace(target.clone());

        target_data.hard_links += 1;
        debug_assert_eq!(
            target_data.hard_links,
            target_inode_ref.inode.links_count().into()
        );

        ext4.write_back_inode(target_inode_ref);

        debug_assert_eq!(
            target_data.hard_links,
            ext4.get_inode_ref(target.id).inode.links_count().into()
        );

        self.reload_from_disk();

        Ok(())
    }

    fn unlink(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        let target = dentry
            .data
            .lock()
            .inode
            .as_ref()
            .ok_or(VfsError::FileNotFound)?
            .clone();
        let target = target.downcast_ref_or_vfserr::<Ext4INode>()?;

        let mut target_wguard = target.data.write();
        let target_data = &mut *target_wguard;

        let fs = unsafe { extend_lifetime(vfs::INode::get_filesystem::<Ext4Fs>(self)) };
        let ext4 = &fs.ops.ext4;

        let my_inode_ref = &mut self.inode;
        let target_inode_ref = &mut target_data.ops.inode;

        ext4.unlink(
            my_inode_ref,
            target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        debug_assert_eq!(
            target_data.hard_links,
            target_inode_ref.inode.links_count().into()
        );
        target_data.hard_links -= 1;
        target_inode_ref
            .inode
            .set_links_count(target_data.hard_links as u16);

        ext4.write_back_inode(target_inode_ref);

        debug_assert_eq!(
            target_data.hard_links,
            ext4.get_inode_ref(target.id).inode.links_count().into()
        );

        self.reload_from_disk();

        Ok(())
    }

    fn mkdir(&mut self, dentry: vfs::DEntryArc, mode: vfs::INodeMode) -> VfsResult<vfs::INodeArc> {
        let result = Self::new_normal(
            vfs::INode::get_filesystem(self),
            vfs::INode::get_id(self),
            dentry.data.lock().name.as_ref(),
            mode,
        );
        self.reload_from_disk();
        result
    }

    fn rmdir(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        let target = dentry
            .data
            .lock()
            .inode
            .as_ref()
            .ok_or(VfsError::FileNotFound)?
            .clone();

        let mut target_wguard = target.downcast_ref_or_vfserr::<Ext4INode>()?.data.write();
        let target_data = &mut *target_wguard;

        let fs = unsafe { extend_lifetime(vfs::INode::get_filesystem::<Ext4Fs>(self)) };
        let ext4 = &fs.ops.ext4;

        let my_inode_ref = &mut self.inode;
        let target_inode_ref = &mut target_data.ops.inode;

        debug_assert!(target_inode_ref.inode.is_dir(), "这是 VFS 层的任务");

        {
            use ext4_rs::{
                BLOCK_SIZE,
                ext4_defs::{Block, Ext4DirEntry, Ext4DirEntryTail},
            };

            // calculate total blocks
            let inode_size = target_inode_ref.inode.size();
            let total_blocks = (inode_size / BLOCK_SIZE as u64) as u32;

            // iterate all blocks
            for iblock in 0..total_blocks {
                // get physical block id of a logical block id
                let search_path = ext4.find_extent(&target_inode_ref, iblock);

                if let Ok(path) = search_path {
                    // get the last path
                    let path = path.path.last().unwrap();

                    // get physical block id
                    let fblock = path.pblock;

                    // load physical block
                    let ext4block = Block::load(&ext4.block_device, fblock as usize * BLOCK_SIZE);
                    let mut offset = 0;

                    // iterate all entries in a block
                    while offset < BLOCK_SIZE - core::mem::size_of::<Ext4DirEntryTail>() {
                        let de: Ext4DirEntry = ext4block.read_offset_as(offset);
                        if !de.unused()
                            && !de.compare_name(ByteStr::new("."))
                            && !de.compare_name(ByteStr::new(".."))
                        {
                            log::debug!(
                                "rmdir err: dir not empty: {}",
                                ByteStr::new(&de.name[..de.get_name_len()])
                            );
                            return Err(VfsError::DirectoryNotEmpty);
                        }
                        offset += de.entry_len() as usize;
                    }
                }
            }
        }

        ext4.truncate_inode(target_inode_ref, 0)?;

        ext4.unlink(
            my_inode_ref,
            target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        ext4.write_back_inode(my_inode_ref);

        debug_assert_eq!(target_data.hard_links, 2);

        self.reload_from_disk();

        Ok(())
    }

    fn rename(
        &mut self,
        new_node: Option<&mut vfs::INodeData>,
        old_name: &ByteStr,
        new_name: ByteString,
        flags: vfs::INodeRenameFlags,
    ) -> VfsResult<()> {
        todo!()
    }

    fn symlink(&mut self, target: ByteString, dentry: vfs::DEntryArc) -> VfsResult<vfs::INodeArc> {
        let parent_id = vfs::INode::get_id(self);
        let mode = vfs::INodeMode::all_permitted() | vfs::INodeMode::SymbolLink;
        let mut result =
            Self::new_normal(vfs::INode::get_filesystem(self), parent_id, &dentry.data.lock().name, mode)?;

        self.reload_from_disk();

        let ret_ref = unsafe {
            (result.get_mut() as *mut vfs::INode)
                .cast::<vfs::INode<Self>>()
                .as_mut_unchecked()
        };
        let ret_data = ret_ref.data.get_mut();

        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;
        let mut inode_ref = &mut ret_data.ops.inode;

        debug_assert_eq!(inode_ref.inode.blocks_count(), 0);
        debug_assert_eq!(inode_ref.inode.size(), 0);

        let target_bytes = target.len();
        if target_bytes < size_of_val(&inode_ref.inode.block) {
            // TODO: 此时其实连页缓存都不需要
            let inline_storge = bytemuck::cast_slice_mut(&mut inode_ref.inode.block);
            inline_storge[..target_bytes].copy_from_slice(&target);
            inline_storge[target_bytes] = b'\0';
            inode_ref.inode.set_flags(0);
        } else {
            todo!()
        }

        // 注意 size 的值不包括末尾的 '\0'
        inode_ref.inode.set_size(target_bytes as u64);
        ret_data.file_size = target_bytes as u64;

        ext4.write_back_inode(&mut inode_ref);

        Ok(result)
    }

    fn read_symlink(&self) -> VfsResult<ByteString> {
        let size = self.inode.inode.size() as usize;
        if size < size_of_val(&self.inode.inode.block) {
            let inline_storge = bytemuck::cast_slice(&self.inode.inode.block);
            let target = ByteStr::new(&inline_storge[..size]);
            Ok(target.to_owned())
        } else {
            todo!()
        }
    }

    fn get_kstat(&self) -> VfsResult<KstatInner> {
        let inode = &self.inode.inode;

        Ok(KstatInner {
            st_uid: inode.uid().into(),
            st_gid: inode.gid().into(),
            st_size: inode.size(),
            st_blksize: ext4_rs::BLOCK_SIZE as u32,
            st_blocks: inode.size() / ext4_rs::BLOCK_SIZE as u64, // FIXME
            st_atime: Time::from_sec(inode.atime().into()),
            st_mtime: Time::from_sec(inode.mtime().into()),
            st_ctime: Time::from_sec(inode.ctime().into()),
            ..Default::default()
        })
    }

    fn evictable_when_mem_ref_zero(&mut self) -> bool {
        true
    }

    fn truncate_shrink_size(&mut self, new_size: u64) {
        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = unsafe { extend_lifetime(&fs.ops.ext4) };
        ext4.truncate_inode(&mut self.inode, new_size).unwrap();
    }
}

impl Ext4INode {
    pub fn new_root(fs: NonNull<vfs::FileSystem>, ext4: &ext4_rs::Ext4) -> vfs::INodeArc {
        let inode = ext4.get_inode_ref(ROOT_INODE);

        vfs::INode::new_raw(ROOT_INODE, fs, vfs::INodeData {
            file_size: inode.inode.size(),
            hard_links: inode.inode.links_count().into(),
            mode: vfs::INodeMode::from_bits_truncate(inode.inode.mode()),
            page_cache: None,
            ops: Self { inode },
        })
    }

    /// Reload the inode from disk to update cached metadata (size, links, etc.)
    /// after an ext4_rs operation that may have modified it on disk.
    fn reload_from_disk(&mut self) {
        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;
        self.inode = ext4.get_inode_ref(vfs::INode::get_id(self));
    }

    fn new_normal(
        fs: &vfs::FileSystem<Ext4Fs>,
        parent_id: u32,
        name: &ByteStr,
        mode: vfs::INodeMode,
    ) -> VfsResult<vfs::INodeArc> {
        let ext4 = &fs.ops.ext4;

        let inode = ext4.create(parent_id, name, mode.bits())?;

        let mut ret = vfs::INode::new(inode.inode_num, NonNull::from_ref(fs), vfs::INodeData {
            file_size: inode.inode.size(),
            hard_links: inode.inode.links_count().into(),
            mode,
            page_cache: None,
            ops: Self { inode },
        });

        unsafe { PageCache::init(ret.get_mut()) };

        Ok(ret)
    }

    fn load_normal(fs: &vfs::FileSystem<Ext4Fs>, id: u32) -> vfs::INodeArc {
        let ext4 = &fs.ops.ext4;

        let inode = ext4.get_inode_ref(id);

        let mut ret = vfs::INode::new(id, NonNull::from_ref(fs), vfs::INodeData {
            file_size: inode.inode.size(),
            hard_links: inode.inode.links_count().into(),
            mode: vfs::INodeMode::from_bits_truncate(inode.inode.mode()),
            page_cache: None,
            ops: Self { inode },
        });

        unsafe { PageCache::init(ret.get_mut()) };

        ret
    }
}
