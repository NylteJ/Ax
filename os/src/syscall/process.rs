use core::{
    ffi::{c_char, c_int},
    num::NonZero,
    pin::Pin,
    ptr::addr_eq,
};

use alloc::sync::Arc;
use bit_field::BitField;
use bitflags::bitflags;
use intrusive_collections::PointerOps;

use crate::{
    filesystem::vfs::{FileFlags, PathToWalk, walk_path},
    memory::uaccess::{CopyError, copy_str_from_user, copy_to_user},
    multitask::{
        current_task,
        task::{Task, TaskArc},
        task_manager::get_task_by_pid,
    },
    syscall::{
        self, SyscallResult,
        filesystem::{PATH_LEN_HINT, PATH_LEN_MAX},
    },
};

use crate::syscall::err_codes::*;

pub use super::basic::exit;

/// * 功能：创建一个子进程；
/// * 输入：
///     - flags: 创建的标志，如SIGCHLD；
///     - stack: 指定新进程的栈，可为0；
///     - ptid: 父线程ID；
///     - tls: TLS线程本地存储描述符；
///     - ctid: 子线程ID；
/// * 返回值：成功则返回子进程的线程ID，失败返回-1；
/// ```c
/// pid_t ret = syscall(SYS_clone, flags, stack, ptid, tls, ctid)
/// ```
pub extern "C" fn clone(
    flags: usize,
    stack: Option<NonZero<usize>>, // Rust 保证其布局和 usize 完全一致
    ptid: usize,
    tls: usize,
    ctid: usize,
) -> SyscallResult {
    let signal = flags.get_bits(0..8) as u8;

    let flags = (flags >> 8).try_into().map_err(|_| EINVAL)?;
    let flags = CloneFlags::from_bits(flags).ok_or(EINVAL)?;

    log::trace!(
        "clone: flags = {flags:?}, signal = {signal:#x} (ignored), stack = {:#x}, ptid = {ptid:#x} (ignored), tls = {tls:#x} (ignored), ctid = {ctid:#x} (ignored)",
        stack.map_or(0, |s| s.get())
    );
    log::error!("TODO: 处理 ptid、tls、ctid、signal");

    let task = unsafe { current_task().as_ref_unchecked() };

    let child = task.fork(flags, stack);

    SyscallResult::success(child.tid)
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct CloneFlags: u32 {
        /// 如果此参数被设定，两个进程将完全共享地址空间；反之，双方地址空间和数据均不共享。
        const VM = 1 << 0;

        /// 如果此参数被设定，子进程将共享父进程的文件系统信息（当前工作目录、根目录、umask）；
        /// 反之，子进程将复制父进程的文件系统信息并独立维护。
        const FS = 1 << 1;

        /// 如果此参数被设定，子进程将共享父进程的文件描述符表；反之，子进程将复制父进程的文件描述符表并独立维护。
        const FILES = 1 << 2;

        const SIGHAND = 1 << 3;
        const PIDFD = 1 << 4;

        /// 如果由 vfork() 创建的进程修改了除用于存储 vfork() 返回值的 pid_t 类型变量之外的任何数据，
        /// 或者从调用 vfork() 的函数中返回，或者在成功调用 _exit 或 exec 函数族中的某个函数之前调用了任何其他函数，
        /// 则行为未定义。
        ///
        /// 其约束的是用户而非操作系统，因此内核可以不做任何额外操作。
        const VFORK = 1 << 6;

        /// 如果此参数被设定，子进程的父进程将继承当前进程的父进程，即创建出的进程变为自身的兄弟进程而非子进程；
        /// 反之，子进程将正常成为子进程。
        const PARENT = 1 << 7;

        /// 如果此参数被设定，子进程将被视为线程而非独立进程，其将被并入父进程所在线程组；
        /// 反之，子进程将被视为独立进程，并且拥有自己的线程组。
        const THREAD = 1 << 8;

        const SYSVSEM = 1 << 10;
        const SETTLS = 1 << 11;
        const PARENT_SETTID = 1 << 12;
        const CHILD_CLEARTID = 1 << 13;
        const DETACHED = 1 << 14;
        const UNTRACED = 1 << 15;
        const CHILD_SETTID = 1 << 16;
        const NEWCGROUP = 1 << 17;
        const NEWUTS = 1 << 18;
        const NEWIPC = 1 << 19;
        const NEWUSER = 1 << 20;
        const NEWPID = 1 << 21;
        const NEWNET = 1 << 22;
        const IO = 1 << 23;
    }
}

/// * 功能：执行一个指定的程序；
/// * 输入：
///     - path: 待执行程序路径名称，
///     - argv: 程序的参数，
///     - envp: 环境变量的数组指针
/// * 返回值：成功不返回，失败返回-1；
/// ```c
/// const char *path, char *const argv[], char *const envp[];
/// int ret = syscall(SYS_execve, path, argv, envp);
/// ```
pub extern "C" fn execve(
    path: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> SyscallResult {
    let path = copy_str_from_user(path, PATH_LEN_HINT, PATH_LEN_MAX)?;

    let task = unsafe { current_task().as_ref_unchecked() };
    let path_to_walk = PathToWalk::auto(path.as_ref(), || {
        task.fs_data.read().current_work_dir.clone()
    })?;

    let file_path = walk_path(path_to_walk)?.ensure()?;

    let file = file_path
        .0
        .data
        .lock()
        .ops
        .open(file_path.1, FileFlags::Read)?;

    log::error!("TODO: argv/envp");

    task.replace_execute(file);

    SyscallResult::success(0)
}

/// * 功能：等待进程改变状态;
/// * 输入：
///     - pid: 指定进程ID，可为-1等待任何子进程；
///     - status: 接收状态的指针；
///     - options: 选项：WNOHANG，WUNTRACED，WCONTINUED；
/// * 返回值：成功则返回进程ID；如果指定了WNOHANG，且进程还未改变状态，直接返回0；失败则返回-1；
/// ```c
/// pid_t pid, int *status, int options;
/// pid_t ret = syscall(SYS_wait4, pid, status, options);
/// ```
// 吐槽一句，它其实是 waitpid
pub extern "C" fn wait4(pid: isize, status: *mut c_int, options: usize) -> SyscallResult {
    let options = WaitOptions::from_bits(options.try_into().map_err(|_| EINVAL)?).ok_or(EINVAL)?;

    let task = unsafe { current_task().as_ref_unchecked() };

    let target_pgid = match pid {
        0 => task.process.pgid,
        ..-1 => pid.unsigned_abs(),
        -1 => return wait4_pred(|_| true, status, options, task),
        1.. => {
            let pid = pid.cast_unsigned();
            let target = get_task_by_pid(pid).ok_or(ECHILD)?;

            if !addr_eq(target.parent_task(), task) {
                return ECHILD;
            }

            loop {
                if let Some((signal, ret_value)) = target.wait_state.lock().status {
                    unsafe { task.children.lock().cursor_mut_from_ptr(&*target).remove() };
                    report_status(status, signal, ret_value)?;
                    return SyscallResult::success(pid);
                }

                if options.contains(WaitOptions::WNOHANG) {
                    return SyscallResult::success(0);
                }

                syscall::basic::yield_(); // 假装睡眠
            }
        }
    };

    wait4_pred(
        |task| task.process.pgid == target_pgid,
        status,
        options,
        task,
    )
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct WaitOptions: u8 {
        /// 如果没有子进程离开，就马上返回 0，而不是阻塞等待
        const WNOHANG = 1 << 0;
        const WUNTRACED = 1 << 1;
        const WCONTINUED = 1 << 3;
    }
}

fn wait4_pred(
    mut pred: impl FnMut(&Task) -> bool,
    status: *mut c_int,
    options: WaitOptions,
    task: &Task,
) -> SyscallResult {
    loop {
        {
            let mut any_possible = false;

            let mut children = task.children.lock();
            let mut cur_child = children.cursor_mut();
            cur_child.move_next();
            while let Some(child) = cur_child.get() {
                debug_assert!(child.tid == child.process.pid);

                if !any_possible {
                    any_possible |= pred(child);
                }

                if let Some((signal, ret_value)) = { child.wait_state.lock().status }
                    && pred(child)
                {
                    let child = cur_child.remove().unwrap();
                    report_status(status, signal, ret_value)?;
                    return SyscallResult::success(child.tid as usize);
                }

                cur_child.move_next();
            }

            if !any_possible {
                // 没有符合要求的子进程
                // log::error!("TODO: POSIX 语义（任意线程都能等待线程组内其它线程的子进程）");
                return ECHILD;
            }
        }

        if options.contains(WaitOptions::WNOHANG) {
            return SyscallResult::success(0);
        }

        syscall::basic::yield_(); // 假装睡眠
    }
}

fn report_status(status: *mut c_int, signal: u8, ret_value: u8) -> Result<(), CopyError> {
    if status.is_null() {
        return Ok(());
    }

    let signal = signal as c_int;
    let ret_value = ret_value as c_int;
    let status_value = signal | (ret_value << 8);
    copy_to_user(status.cast(), &status_value.to_ne_bytes())
}

/// * 功能：获取父进程ID；
/// * 输入：系统调用ID；
/// * 返回值：成功返回父进程ID；
/// ```c
/// pid_t ret = syscall(SYS_getppid);
/// ```
pub extern "C" fn getppid() -> usize {
    let task = unsafe { current_task().as_ref_unchecked() };
    task.parent_task().process.pid
}

/// * 功能：获取进程ID；
/// * 输入：系统调用ID；
/// * 返回值：成功返回进程ID；
/// ```c
/// pid_t ret = syscall(SYS_getpid);
/// ```
pub extern "C" fn getpid() -> usize {
    let task = unsafe { current_task().as_ref_unchecked() };
    task.process.pid
}

/// * 功能：获取线程ID；
/// * 输入：系统调用ID；
/// * 返回值：成功返回线程ID；
/// ```c
/// pid_t ret = syscall(SYS_gettid);
/// ```
pub extern "C" fn gettid() -> usize {
    let task = unsafe { current_task().as_ref_unchecked() };
    task.tid
}
