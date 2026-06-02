use core::ffi::c_void;
use core::slice;

use crate::basic::timer::cpu_tick_to_us;
use crate::basic::timer::get_cpu_tick;
use crate::memory::uaccess::copy_one_to_user;
use crate::memory::uaccess::copy_to_user;
use crate::syscall::SyscallResult;

use crate::syscall::err_codes::*;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// ### #define SYS_gettimeofday 169
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

    const DATA: &[u8] = concat_bytes!(
        b"Ax\0",        // sysname
        b"Ax\0",        // nodename
        b"0.0.1\0",     // release
        b"0.0.1\0",     // version
        b"Unknown\0",   // machine
        b"localhost\0", // domainname
    );

    copy_to_user(uts.cast(), DATA)?;

    SyscallResult::success(0)
}
