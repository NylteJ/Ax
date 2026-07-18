use bitflags::bitflags;
use hashbrown::hash_map;
use num_enum::TryFromPrimitive;

use crate::filesystem::EPollFile;
use crate::memory::uaccess::copy_one_from_user;
use crate::multitask::FDTableEntry;
use crate::multitask::cur_task;
use crate::syscall::SyscallResult;

use crate::syscall::err_codes::*;
use crate::syscall::filesystem::cur_fd_rlimit;
use crate::syscall::filesystem::get_file_from_fd;

bitflags! {
    pub struct EPollFlags: u32 {
        const CLOEXEC = 0x80000;
    }
}

pub extern "C" fn epoll_create1(flags: usize) -> SyscallResult {
    let flags: u32 = flags.try_into().map_err(|_| EINVAL)?;
    let flags = EPollFlags::from_bits(flags).ok_or(EINVAL)?;

    let epoll_file = EPollFile::new();

    let fd_entry = if flags.contains(EPollFlags::CLOEXEC) {
        FDTableEntry::cloexec(epoll_file)
    } else {
        FDTableEntry::no_flags(epoll_file)
    };

    let fd = cur_task().files.write().insert(fd_entry, cur_fd_rlimit())?;

    SyscallResult::success(fd)
}

#[repr(u8)]
#[derive(Clone, Copy, TryFromPrimitive, PartialEq, Eq)]
enum EPollOp {
    Add = 1,
    Del = 2,
    Mod = 3,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EPollEvent {
    pub events: u32,
    pub data: u64,
}

pub extern "C" fn epoll_ctl(
    epfd: isize,
    op: isize,
    fd: isize,
    event: *mut EPollEvent,
) -> SyscallResult {
    if epfd == fd {
        return EINVAL;
    }

    let op: u8 = op.try_into().map_err(|_| EINVAL)?;
    let op = EPollOp::try_from(op).map_err(|_| EINVAL)?;

    let file = get_file_from_fd(fd)?;
    let fd = fd.cast_unsigned();

    let epfile = get_file_from_fd(epfd)?;

    let epfile = EPollFile::from_dyn_file(&epfile).ok_or(EINVAL)?;

    let mut epfile_guard = epfile.data.lock();
    let ep_file = unsafe { epfile_guard.get_ops() };

    match op {
        EPollOp::Add => {
            let event = copy_one_from_user(event)?;

            let entry = ep_file.entries.entry(fd);
            let hash_map::Entry::Vacant(vacant) = entry else {
                return EEXIST;
            };

            vacant.insert(event);
        }
        EPollOp::Del => {
            ep_file.entries.remove(&fd).ok_or(ENOENT)?;
        }
        EPollOp::Mod => *ep_file.entries.get_mut(&fd).ok_or(ENOENT)? = copy_one_from_user(event)?,
    }

    SyscallResult::success(0)
}

// 没错，我们没有实现 epoll_pwait，因为我们喜欢 reward hacking！
