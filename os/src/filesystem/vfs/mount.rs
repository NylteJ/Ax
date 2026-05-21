use core::{
    mem::{forget, transmute},
    ops::Deref,
    pin::Pin,
    ptr::{NonNull, addr_eq},
};

use alloc::sync::{Arc, UniqueArc};
use intrusive_collections::{LinkedList as IntrusiveLinkedList, LinkedListLink, intrusive_adapter};

use crate::{
    basic::{extend_lifetime, extend_lifetime_mut, sync::RwLock},
    filesystem::vfs::{DEntryArc, FileSystem, Path},
};

#[derive(Debug)]
pub struct MountPoint {
    parent: Path,
    mounts: RwLock<IntrusiveLinkedList<MountAdapter>>,
}

#[derive(Debug)]
pub struct Mount {
    pub sub_fs: Pin<Arc<FileSystem>>,

    /// 挂载到的子文件系统目录项，注意不一定是根目录
    pub mount_to: DEntryArc,

    pub mount_point: Arc<MountPoint>,

    pub(self) link: LinkedListLink,
}

unsafe impl Send for Mount {}
unsafe impl Sync for Mount {}

intrusive_adapter!(MountAdapter = Arc<Mount>: Mount { link => LinkedListLink });

impl MountPoint {
    pub fn mount(parent: Path, sub_fs: Pin<Arc<FileSystem>>, mount_to: DEntryArc) {
        let mut guard = unsafe { extend_lifetime(parent.0.deref()) }.data.lock();

        let (arc, is_first) = if let Some(mount) = &guard.mount {
            let arc = unsafe { Arc::from_raw(mount.as_ptr()) };
            forget(arc.clone());
            (arc, false)
        } else {
            (
                Arc::new(MountPoint {
                    parent,
                    mounts: RwLock::new(IntrusiveLinkedList::new(MountAdapter::new())),
                }),
                true,
            )
        };

        let self_ref = unsafe { extend_lifetime(arc.as_ref()) };

        let new_mount = Arc::new(Mount {
            sub_fs,
            mount_to,
            mount_point: arc,
            link: LinkedListLink::new(),
        });

        self_ref.mounts.write().push_front(new_mount);
        if is_first {
            guard.mount.replace(NonNull::from_ref(self_ref));
        }
    }

    pub unsafe fn unmount(&self, mount: &Mount) {
        debug_assert!(mount.link.is_linked());

        unsafe { self.mounts.write().cursor_mut_from_ptr(mount).remove() }.unwrap();
    }

    pub unsafe fn mount_root(fs: Pin<Arc<FileSystem>>) -> Arc<Mount> {
        let root_dentry = fs.root_dentry.clone();

        let mut mp = UniqueArc::new(MountPoint {
            parent: Path(root_dentry.clone(), unsafe {
                // SAFETY: 这实际上并不安全，但我不愿意在此花更多时间了
                transmute(NonNull::<Mount>::dangling())
            }),
            mounts: RwLock::new(IntrusiveLinkedList::new(MountAdapter::new())),
        });

        let mp_ref = unsafe { extend_lifetime_mut(mp.as_mut()) };
        let data_ptr = root_dentry.data.as_ptr();

        let mnt = Arc::new(Mount {
            sub_fs: fs,
            mount_to: root_dentry,
            mount_point: UniqueArc::into_arc(mp),
            link: LinkedListLink::new(),
        });

        mp_ref.mounts.get_mut().push_front(mnt.clone());
        unsafe { (&raw mut mp_ref.parent.1).write(mnt.clone()) };
        unsafe { data_ptr.as_mut_unchecked() }
            .mount
            .replace(NonNull::from_mut(mp_ref));

        mnt
    }

    pub fn get_cur_mount(&self) -> Arc<Mount> {
        self.mounts.read().front().clone_pointer().unwrap()
    }

    pub fn get_parent(&self) -> Path {
        self.parent.clone()
    }
}

impl Drop for MountPoint {
    fn drop(&mut self) {
        let old_value = self.parent.0.data.lock().mount.take();

        debug_assert!(old_value.is_some_and(|val| addr_eq(self, val.as_ptr())));
    }
}
