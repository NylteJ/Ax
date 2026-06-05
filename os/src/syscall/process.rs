use core::{
    ffi::{c_char, c_int, c_long},
    mem::MaybeUninit,
    num::NonZero,
    pin::Pin,
    ptr::addr_eq,
    sync::atomic::Ordering,
};

use alloc::{sync::Arc, vec::Vec};
use bit_field::BitField;
use bitflags::bitflags;
use intrusive_collections::PointerOps;
use rand::Rng;

use crate::{
    basic::timer::{get_cpu_tick, us_to_cpu_tick},
    filesystem::{
        devfs::drivers::URANDOM_DRIVER,
        vfs::{FileFlags, PathToWalk, walk_path},
    },
    memory::{
        STACK_ALIGN,
        uaccess::{
            CopyError, copy_from_user, copy_from_user_uninit, copy_str_from_user, copy_to_user,
        },
    },
    multitask::{
        current_task,
        signal::SignalId,
        task::{AuxId, AuxiliaryEntry, Task, TaskArc, USER_STACK_BYTES},
        task_manager::get_task_by_pid,
    },
    syscall::{
        self, SyscallResult,
        common_types::TimeSpec,
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

    flags.check_validity()?;

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

        /// 如果此参数被设定，子进程将共享父进程的信号处理程序；反之，子进程将复制父进程的信号处理程序并独立维护。
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

impl CloneFlags {
    pub fn check_validity(&self) -> SyscallResult {
        let bad = (self.contains(CloneFlags::THREAD) && !self.contains(CloneFlags::SIGHAND))
            || (self.contains(CloneFlags::SIGHAND) && !self.contains(CloneFlags::VM));

        if bad {
            EINVAL
        } else {
            SyscallResult::success(0)
        }
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

    // 接下来从用户侧读取 argv 和 envp
    // TODO: 这里其实也能优化，提前把新的栈内存分配好，然后直接拷贝到栈内存，以减少一次复制并完全避免 Vec 扩容
    let mut data = Vec::new();
    let envp_idxs = read_str_arr_from_user(envp, 32, 2048, &mut data)?;
    let argv_idxs = read_str_arr_from_user(argv, 16, 512, &mut data)?;

    let argc = argv_idxs.len();

    // AT_RANDOM 所需的 16 字节随机数据我们也放在 data 里
    let random_idx = data.len();
    let mut random_data = [0; 16];
    URANDOM_DRIVER.engine.lock().fill_bytes(&mut random_data);
    data.extend(random_data);

    // 由于执行 replace_execute 后便不能再回头，这里需要提前检查，而且后面就不能再返回失败
    // 注意这里的大小不包括 auxv，因为 auxv 完全由我们控制
    let args_bytes_unaligned = {
        let data_bytes = data.len();
        let ptrs_bytes = (envp_idxs.len() + 1 + argv_idxs.len() + 1) * size_of::<*const c_char>();
        let argc_bytes = size_of::<usize>();

        data_bytes + ptrs_bytes + argc_bytes
    };

    const MAX_ARGS_BYTES: usize = USER_STACK_BYTES / 4;
    if args_bytes_unaligned > MAX_ARGS_BYTES {
        // 这实际上只是一个估算，但一般来讲这个限制都是 MB 级的，所以并不会有多大影响
        log::warn!(
            "用户侧传入的 execve 参数过大（{args_bytes_unaligned} 字节 > {MAX_ARGS_BYTES} 字节）",
        );
        return E2BIG;
    }

    // 半成品 auxv（缺少 AT_RANDOM 和 AT_NULL）
    let auxv = task.replace_execute(file)?;
    debug_assert!(auxv.iter().all(|entry| entry.0 != AuxId::AT_NULL));

    let auxv_bytes = (auxv.len() + 2) * size_of::<AuxiliaryEntry>();
    let data_bytes_unaligned = args_bytes_unaligned + auxv_bytes;
    let data_bytes_padding = STACK_ALIGN - data_bytes_unaligned % STACK_ALIGN;
    let data_bytes = data_bytes_unaligned + data_bytes_padding;

    debug_assert!(data_bytes <= USER_STACK_BYTES);

    let current_sp = &mut unsafe { task.get_trap_context().cast_mut().as_mut_unchecked() }
        .regs
        .sp;
    debug_assert_eq!(*current_sp % STACK_ALIGN, 0);

    let data_end = *current_sp;

    // 对齐填充
    *current_sp -= data_bytes_padding;

    // argv、envp 的具体数据，以及 AT_RANDOM 的随机数据
    *current_sp -= data.len();
    let data_begin = *current_sp;
    copy_to_user(data_begin as *mut u8, bytemuck::cast_slice(&data)).expect("此时栈不应溢出");

    // auxv（顺带一提，AT_RANDOM 基本上是初始化 glibc 所必需的，我也不知道为什么）
    let mut auxv = auxv;
    auxv.push(AuxiliaryEntry(AuxId::AT_RANDOM, random_idx + data_begin));
    auxv.push(AuxiliaryEntry(AuxId::AT_NULL, 0));
    log::trace!("final auxv: {auxv:?}");
    debug_assert_eq!(auxv_bytes, size_of_val(auxv.as_slice()));

    *current_sp -= auxv_bytes;
    copy_to_user(*current_sp as *mut u8, bytemuck::cast_slice(&auxv)).expect("此时栈不应溢出");

    // argv、envp 的指针数组，指向之前写入的具体数据
    push_indexes(envp_idxs, data_begin, current_sp);
    push_indexes(argv_idxs, data_begin, current_sp);

    // argc
    *current_sp -= size_of::<usize>();
    copy_to_user(*current_sp as *mut u8, &argc.to_ne_bytes()).expect("此时栈不应溢出");

    debug_assert_eq!(*current_sp % STACK_ALIGN, 0);
    debug_assert_eq!(*current_sp + data_bytes, data_end);

    SyscallResult::success(0) // 这个返回值其实没什么意义，它会成为新程序启动时的 a0，但除此之外不会有更多作用
}

fn read_str_arr_from_user(
    u_src: *const *const c_char,
    entry_len_hint: usize,
    entry_len_max: usize,
    data: &mut Vec<u8>,
) -> Result<Vec<usize>, CopyError> {
    let mut indexes = Vec::new();

    let mut cur_ptr = 1 as *const c_char;
    let mut src = u_src;
    loop {
        let cur_ptr_bytes = unsafe {
            (&raw mut cur_ptr)
                .cast::<[u8; size_of::<*const c_char>()]>()
                .as_mut_unchecked()
        };
        copy_from_user(cur_ptr_bytes, src.cast())?;

        if cur_ptr.is_null() {
            break;
        }

        // TODO: 这里可以优化，用底层版本的函数直接读到 data 中，避免两次复制
        let arg = copy_str_from_user(cur_ptr, entry_len_hint, entry_len_max)?;
        indexes.push(data.len());
        data.reserve(arg.len() + 1);
        data.extend(arg.0);
        data.push(b'\0');

        src = unsafe { src.add(1) };
    }

    Ok(indexes)
}

fn push_indexes(mut indexes: Vec<usize>, data_begin: usize, current_sp: &mut usize) {
    for idx in &mut indexes {
        *idx += data_begin;
    }
    indexes.push(0);
    *current_sp -= indexes.len() * size_of::<usize>();
    copy_to_user(*current_sp as *mut u8, bytemuck::cast_slice(&indexes)).expect("此时栈不应溢出");
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

                    unsafe { current_task().as_ref_unchecked() }
                        .time_stats
                        .lock()
                        .update_children_times(&child.time_stats.lock());

                    return SyscallResult::success(child.tid);
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

/// * 功能：执行线程睡眠，sleep()库函数基于此系统调用；
/// * 输入：睡眠的时间间隔；
/// ```c
/// struct timespec {
/// 	time_t tv_sec;        /* 秒 */
/// 	long   tv_nsec;       /* 纳秒, 范围在0~999999999 */
/// };
/// ```
/// * 返回值：成功返回0，失败返回-1;
/// ```c
/// const struct timespec *req, struct timespec *rem;
/// int ret = syscall(SYS_nanosleep, req, rem);
/// ```
pub extern "C" fn nanosleep(req: *const TimeSpec, rem: *mut TimeSpec) -> SyscallResult {
    let mut k_req = MaybeUninit::uninit();
    copy_from_user_uninit(k_req.as_bytes_mut(), req.cast())?;
    let req: TimeSpec = unsafe { k_req.assume_init() };

    // 因为我们还没有实现信号系统或其它可能导致睡眠被打断的机制，此处 rem 暂时无用
    let _ = rem;

    let TimeSpec {
        tv_sec: seconds @ 0..,
        tv_nsec: nanos @ 0..=999999999,
    } = req
    else {
        // EINVAL The value in the tv_nsec field was not in the range 0 to 999999999 or tv_sec was negative.
        return EINVAL;
    };

    let nanos = seconds * 1000000000 + nanos;
    let us = nanos.div_ceil(1000);

    let target_tick = get_cpu_tick() + us_to_cpu_tick(us as usize);

    loop {
        let cur_tick = get_cpu_tick();

        if cur_tick >= target_tick {
            break;
        }

        const BUSY_WAIT_THRESHOLD: usize = 1024;

        if cur_tick + BUSY_WAIT_THRESHOLD < target_tick {
            syscall::basic::yield_(); // 假装睡眠
        }
    }

    SyscallResult::success(0)
}

// FIXME
pub extern "C" fn set_tid_address() -> usize {
    0
}

pub extern "C" fn exit_group(code: isize) -> ! {
    let cur_task = unsafe { current_task().as_ref_unchecked() };

    let is_already_exiting = cur_task.process.is_exiting.swap(true, Ordering::Relaxed);
    if !is_already_exiting {
        for task in &*cur_task.process.tasks.read() {
            // 注意这里也会遍历到自己，但没什么关系，因为我们再也不会回用户态了
            task.send_signal(SignalId::SIGKILL);
        }
    }

    exit(code)
}
