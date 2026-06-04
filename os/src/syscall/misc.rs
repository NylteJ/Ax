use core::ffi::c_void;
use core::slice;
use core::sync::atomic::Ordering;

use crate::basic::timer::cpu_tick_to_us;
use crate::basic::timer::get_cpu_tick;
use crate::memory::uaccess::copy_one_to_user;
use crate::memory::uaccess::copy_slice_to_user;
use crate::memory::uaccess::copy_to_user;
use crate::multitask::current_task;
use crate::multitask::task_manager::get_task_by_pid;
use crate::syscall::SyscallResult;

use crate::syscall::err_codes::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// * 功能：获取时间；
/// * 输入： timespec结构体指针用于获得时间值；
/// * 返回值：成功返回0，失败返回-1;
/// ```c
/// struct timespec *ts;
/// int ret = syscall(SYS_gettimeofday, ts, 0);
/// ```
pub extern "C" fn gettimeofday(tv: *mut TimeVal, tz: usize) -> SyscallResult {
    if tz != 0 {
        log::error!("gettimeofday 不支持时区");
        return EINVAL;
    }

    if !tv.is_null() {
        let us = cpu_tick_to_us(get_cpu_tick());
        let time_val = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };

        copy_one_to_user(tv, &time_val)?;
    }

    SyscallResult::success(0)
}

/// * 功能：打印系统信息；
/// * 输入：utsname结构体指针用于获得系统信息数据；
/// * 返回值：成功返回0，失败返回-1;
/// ```c
/// struct utsname *uts;
/// int ret = syscall(SYS_uname, uts);
/// ```
pub extern "C" fn uname(uts: *mut c_void) -> SyscallResult {
    // https://www.man7.org/linux/man-pages/man2/uname.2.html
    // struct utsname {
    //     char sysname[];    /* Operating system name (e.g., "Linux") */
    //     char nodename[];   /* Name within communications network
    //                           to which the node is attached, if any */
    //     char release[];    /* Operating system release
    //                           (e.g., "2.6.28") */
    //     char version[];    /* Operating system version */
    //     char machine[];    /* Hardware type identifier */
    // #ifdef _GNU_SOURCE
    //     char domainname[]; /* NIS or YP domain name */
    // #endif
    // };

    type Entry = [u8; 65];

    const DATA: [Entry; 6] = {
        const fn entry(str: &[u8]) -> Entry {
            let mut entry = [0; 65];
            assert!(str.len() < entry.len());
            entry.split_at_mut(str.len()).0.copy_from_slice(str);
            entry
        }

        [
            entry(b"Ax"),        // sysname
            entry(b"Ax"),        // nodename
            entry(b"5 (fake)"),  // release
            entry(b"0.0.1"),     // version
            entry(b"Unknown"),   // machine
            entry(b"localhost"), // domainname
        ]
    };

    copy_slice_to_user(uts.cast(), &DATA)?;

    SyscallResult::success(0)
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct RobustListHead {
    list: *mut (),
    futex_offset: usize,
    list_op_pending: *mut (),
}

pub extern "C" fn set_robust_list(head: *mut RobustListHead, len: usize) -> SyscallResult {
    if len != size_of::<RobustListHead>() {
        return EINVAL;
    }

    let cur_task = unsafe { current_task().as_ref_unchecked() };

    cur_task.robust_list_head.store(head, Ordering::Release);

    SyscallResult::success(0)
}

pub extern "C" fn get_robust_list(
    pid: isize,
    head_ptr: *mut *mut RobustListHead,
    len_ptr: *mut usize,
) -> SyscallResult {
    if pid < 0 {
        return ESRCH;
    }
    let pid = pid.cast_unsigned();

    let task = if pid == 0 {
        unsafe { current_task().as_ref_unchecked() }
    } else {
        &get_task_by_pid(pid).ok_or(ESRCH)?
    };

    let head = task.robust_list_head.load(Ordering::Acquire);
    copy_one_to_user(head_ptr, &head)?;
    copy_one_to_user(len_ptr, &size_of::<RobustListHead>())?;

    SyscallResult::success(0)
}
