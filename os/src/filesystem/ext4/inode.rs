use core::ptr::NonNull;

use alloc::borrow::ToOwned;
use ext4_rs::ext4_defs::ROOT_INODE;

use crate::{
    basic::bstr::{ByteStr, ByteString},
    filesystem::{
        ext4::{dentry::Ext4DEntry, filesystem::Ext4Fs},
        vfs::{self, INodeOps, PageCache, VfsError, VfsResult},
    },
    syscall::filesystem::{KstatInner, Time},
};

pub struct Ext4INode {
    pub inode: ext4_rs::ext4_defs::Ext4Inode,
}

impl INodeOps for Ext4INode {
    fn create(&mut self, mode: vfs::INodeMode, dentry: vfs::DEntryArc) -> VfsResult<vfs::INodeArc> {
        Self::new_normal(
            vfs::INode::get_filesystem(self),
            vfs::INode::get_id(self),
            dentry.data.lock().name.as_ref(),
            mode | vfs::INodeMode::RegularFile,
        )
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
        let mut my_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode: self.inode,
            inode_num: vfs::INode::get_id(self),
        };
        let mut target_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode: target
                .downcast_ref_or_vfserr::<Ext4INode>()?
                .data
                .read()
                .ops
                .inode,
            inode_num: target.id,
        };

        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;

        ext4.link(
            &mut my_inode_ref,
            &mut target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        dentry.data.lock().inode.replace(target.clone());

        target.data.write().hard_links += 1;
        debug_assert_eq!(
            target.data.read().hard_links,
            target_inode_ref.inode.links_count().into()
        );

        ext4.write_back_inode(&mut target_inode_ref);

        debug_assert_eq!(
            target.data.read().hard_links,
            ext4.get_inode_ref(target.id).inode.links_count().into()
        );

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

        let mut my_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode: self.inode,
            inode_num: vfs::INode::get_id(self),
        };
        let mut target_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode_num: target.id,
            inode: target
                .downcast_ref_or_vfserr::<Ext4INode>()?
                .data
                .read()
                .ops
                .inode,
        };

        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;

        ext4.unlink(
            &mut my_inode_ref,
            &mut target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        target.data.write().hard_links -= 1;
        debug_assert_eq!(
            target.data.read().hard_links,
            target_inode_ref.inode.links_count().into()
        );

        ext4.write_back_inode(&mut target_inode_ref);

        debug_assert_eq!(
            target.data.read().hard_links,
            ext4.get_inode_ref(target.id).inode.links_count().into()
        );

        Ok(())
    }

    fn mkdir(&mut self, dentry: vfs::DEntryArc, mode: vfs::INodeMode) -> VfsResult<vfs::INodeArc> {
        Self::new_normal(
            vfs::INode::get_filesystem(self),
            vfs::INode::get_id(self),
            dentry.data.lock().name.as_ref(),
            mode | vfs::INodeMode::Directory,
        )
    }

    fn rmdir(&mut self, dentry: &vfs::DEntryArc) -> VfsResult<()> {
        let target = dentry
            .data
            .lock()
            .inode
            .as_ref()
            .ok_or(VfsError::FileNotFound)?
            .clone();

        let mut my_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode: self.inode,
            inode_num: vfs::INode::get_id(self),
        };
        let mut target_inode_ref = ext4_rs::ext4_defs::Ext4InodeRef {
            inode_num: target.id,
            inode: target
                .downcast_ref_or_vfserr::<Ext4INode>()?
                .data
                .read()
                .ops
                .inode,
        };

        if !target_inode_ref.inode.is_dir() {
            return Err(VfsError::NotADirectory);
        }

        let fs = vfs::INode::get_filesystem::<Ext4Fs>(self);
        let ext4 = &fs.ops.ext4;

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

        ext4.truncate_inode(&mut target_inode_ref, 0)?;

        ext4.unlink(
            &mut my_inode_ref,
            &mut target_inode_ref,
            dentry.data.lock().name.as_ref(),
        )?;

        ext4.write_back_inode(&mut my_inode_ref);

        debug_assert_eq!(target.data.read().hard_links, 2);

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

    fn get_kstat(&self) -> VfsResult<KstatInner> {
        let inode = &self.inode;

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
}

impl Ext4INode {
    pub fn new_root(fs: NonNull<vfs::FileSystem>, ext4: &ext4_rs::Ext4) -> vfs::INodeArc {
        let inode = ext4.get_inode_ref(ROOT_INODE).inode;

        vfs::INode::new_raw(ROOT_INODE, fs, vfs::INodeData {
            file_size: inode.size(),
            hard_links: inode.links_count().into(),
            mode: vfs::INodeMode::from_bits_truncate(inode.mode()),
            page_cache: None,
            ops: Self { inode },
        })
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
            ops: Self { inode: inode.inode },
        });

        unsafe { PageCache::init(ret.get_mut()) };

        Ok(ret)
    }

    fn load_normal(fs: &vfs::FileSystem<Ext4Fs>, id: u32) -> vfs::INodeArc {
        let ext4 = &fs.ops.ext4;

        let inode = ext4.get_inode_ref(id).inode;

        let mut ret = vfs::INode::new(id, NonNull::from_ref(fs), vfs::INodeData {
            file_size: inode.size(),
            hard_links: inode.links_count().into(),
            mode: vfs::INodeMode::from_bits_truncate(inode.mode()),
            page_cache: None,
            ops: Self { inode },
        });

        unsafe { PageCache::init(ret.get_mut()) };

        ret
    }
}
