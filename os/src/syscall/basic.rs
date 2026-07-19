use core::{mem::replace, num::NonZero};

use num_enum::TryFromPrimitive;

use crate::{
    basic::{
        cleanup_and_shutdown,
        shared_with_asm::SharedWithAsm,
        timer::{
            check_all_timers, cpu_tick_to_us, get_cpu_tick, get_itimer_real, set_itimer_real,
            us_to_cpu_tick,
        },
    },
    memory::uaccess::{copy_one_from_user, copy_one_to_user},
    multitask::{
        cur_task, current_task,
        signal::SignalId,
        task::{INIT_PID, Task, TaskArc},
        task_manager::get_task_by_tid,
        task_scheduler::TASK_SCHEDULER,
    },
    println,
    syscall::{SyscallResult, misc::TimeVal},
};

use crate::syscall::err_codes::*;

pub fn exit_kernel(ec: isize, signal: SignalId) -> ! {
    log::trace!("Task {} exited with code {1} ({1:#x}).", cur_task().tid, ec);

    let cur = unsafe { TaskArc::from_asm(current_task()) };

    // FIXME: temp
    #[cfg(debug_assertions)]
    if cur.tid == INIT_PID {
        let mut raw = crate::trap::trap_handler::UNKNOWN_SYSCALLS
            .lock()
            .drain()
            .collect::<alloc::vec::Vec<_>>();

        raw.sort_unstable_by_key(|(_, count)| -count.cast_signed());

        let list = raw
            .into_iter()
            .map(|(id, count)| {
                alloc::format!(
                    "{} (id = {id}): {count} 次",
                    super::SYSCALL_ID_NAME_MAP.get(id).unwrap_or(&"???")
                )
            })
            .collect::<alloc::vec::Vec<_>>();

        log::info!("所有未知系统调用：\n{list:#?}")
    }
    // debug_assert!(cur.tid != INIT_PID, "init 进程不应退出");

    // FIXME...?
    if cur.tid == INIT_PID {
        println!("init 进程退出，关机");
        cleanup_and_shutdown();
    }

    cur.on_exit(signal, ec as u8);

    let Some(next_task) = TASK_SCHEDULER.lock().pick_next() else {
        log::info!("所有用户程序运行完毕，关机");
        cleanup_and_shutdown();
    };

    // 注意只在不关机时 drop 当前任务，因为现在使用的页表还是当前任务的页表，drop 掉了其内存就可能被重用（特别注意线程在销毁前不需要 wait），
    // 导致清理任务出现各种问题。实际上连目前我们所在的内核栈都有可能被重用。
    // 不关机时 drop 因为下一步就是 switch_to，它不进行内存分配，所以没问题
    drop(cur);

    Task::switch_to(next_task);
    unreachable!();
}

/// * 功能：触发进程终止，无返回值；
/// * 输入：终止状态值；
/// * 返回值：无返回值；
/// ```c
/// int ec;
/// syscall(SYS_exit, ec);
/// ```
pub extern "C" fn exit(ec: isize) -> ! {
    exit_kernel(ec, SignalId::SIGINVAL);
}

pub extern "C" fn yield_() -> isize {
    // log::trace!("Task {} yielded.", cur_task().tid);

    // FIXME: 这实际上是目前用 yield 模拟 sleep 所需要的变通方案
    //        正统 sleep 会检查这一点
    check_all_timers();

    let mut scheduler = TASK_SCHEDULER.lock();
    scheduler.add_task(unsafe { TaskArc::from_asm(current_task()) });
    let next_task = scheduler.pick_next().unwrap();
    drop(scheduler);

    Task::switch_to_maybe_current(next_task);

    0
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Tms {
    tms_utime: usize,
    tms_stime: usize,
    tms_cutime: usize,
    tms_cstime: usize,
}

/// * 功能：获取进程时间；
/// * 输入：tms结构体指针，用于获取保存当前进程的运行时间数据；
/// * 返回值：成功返回已经过去的滴答数，失败返回-1;
/// ```c
/// struct tms *tms;
/// clock_t ret = syscall(SYS_times, tms);
/// ```
pub extern "C" fn times(tms: *mut Tms) -> SyscallResult {
    if !tms.is_null() {
        let tms_data = {
            let time_stats = cur_task().time_stats.read();
            Tms {
                tms_utime: time_stats.user_time,
                tms_stime: time_stats.kernel_time,
                tms_cutime: time_stats.children_user_time,
                tms_cstime: time_stats.children_kernel_time,
            }
        };

        copy_one_to_user(tms, &tms_data)?;
    }

    SyscallResult::success(get_cpu_tick())
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RLimit {
    pub rlim_cur: usize,
    pub rlim_max: usize,
}
pub const RLIM_INFINITY: usize = usize::MAX;

impl RLimit {
    pub const fn unlimited() -> Self {
        Self {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        }
    }
}

#[repr(u8)]
#[derive(Debug, TryFromPrimitive)]
#[allow(non_camel_case_types)]
pub enum RLimitResource {
    // https://www.man7.org/linux/man-pages/man2/prlimit64.2.html
    /// 这是进程虚拟内存（地址空间）的最大大小。限制值以字节为单位指定，并向下取整到系统页面大小。
    /// 此限制会影响 brk(2)、mmap(2) 和 mremap(2) 的调用，当超过此限制时，这些调用将失败并返回错误 ENOMEM。
    /// 此外，自动栈扩展会失败（并且如果没有通过 sigaltstack(2) 提供备用栈，则会生成 SIGSEGV 信号终止进程）。
    /// 由于该值是一个 long 类型，在 32 位 long 的机器上，此限制最大为 2 GiB，或者该资源为无限制。
    RLIMIT_AS = 9,
    /// 这是进程可能转储的核心文件（参见 core(5)）的最大字节大小。
    /// 当为 0 时，不创建任何核心转储文件。当非零时，更大的转储将被截断到此大小。
    RLIMIT_CORE = 4,
    /// 这是进程可以消耗的 CPU 时间量的限制，以秒为单位。当进程达到软限制时，会向其发送 SIGXCPU 信号。
    /// 此信号的默认操作是终止进程。但是，可以捕获该信号，并且处理程序可以将控制权返回给主程序。
    /// 如果进程继续消耗 CPU 时间，它将每秒收到一次 SIGXCPU，直到达到硬限制，此时会向其发送 SIGKILL。
    /// （最后这一点描述的是 Linux 行为。对于达到软限制后继续消耗 CPU 时间的进程如何处理，各实现有所不同。需要捕获此信号的可移植应用程序应在首次收到 SIGXCPU 时执行有序终止。）
    RLIMIT_CPU = 0,
    /// 这是进程数据段（已初始化数据、未初始化数据和堆）的最大大小。限制值以字节为单位指定，并向下取整到系统页面大小。
    /// 此限制会影响 brk(2)、sbrk(2) 以及（自 Linux 4.7 起）mmap(2) 的调用，当遇到此资源的软限制时，这些调用将失败并返回错误 ENOMEM。
    RLIMIT_DATA = 2,
    /// 这是进程可以创建的文件的最大字节大小。尝试将文件扩展超过此限制会导致传递 SIGXFSZ 信号。
    /// 默认情况下，此信号会终止进程，但进程可以改为捕获此信号，在这种情况下，相关的系统调用（例如 write(2)、truncate(2)）将失败并返回错误 EFBIG。
    RLIMIT_FSIZE = 1,
    /// 这是此进程可以建立的 flock(2) 锁和 fcntl(2) 租约的总数限制。
    RLIMIT_LOCKS = 10,
    /// 这是可以锁定到 RAM 中的内存的最大字节数。此限制实际上向下取整到系统页面大小的最近倍数。
    /// 此限制影响 mlock(2)、mlockall(2) 以及 mmap(2) 的 MAP_LOCKED 操作。
    /// 自 Linux 2.6.9 起，它还影响 shmctl(2) 的 SHM_LOCK 操作，该操作为调用进程的实际用户 ID 可能锁定的共享内存段（参见 shmget(2)）中的总字节数设置了一个最大值。
    /// shmctl(2) SHM_LOCK 锁与通过 mlock(2)、mlockall(2) 和 mmap(2) MAP_LOCKED 建立的每个进程的内存锁分开计数；一个进程在这两个类别中的每一个类别中可以锁定最多达此限制的字节数。
    ///
    /// 在 Linux 2.6.9 之前，此限制控制特权进程可以锁定的内存量。
    /// 自 Linux 2.6.9 起，对特权进程可以锁定的内存量没有限制，相反，此限制管理非特权进程可以锁定的内存量。
    RLIMIT_MEMLOCK = 8,
    /// 这是可以为调用进程的实际用户 ID 分配的 POSIX 消息队列字节数的限制。此限制对 mq_open(3) 强制执行。
    /// 用户创建的每个消息队列（在删除之前）根据以下公式计入此限制：
    ///
    /// 自 Linux 3.5 起：
    /// `bytes = attr.mq_maxmsg * sizeof(struct msg_msg) + MIN(attr.mq_maxmsg, MQ_PRIO_MAX) * sizeof(struct posix_msg_tree_node) + /* 开销 */ attr.mq_maxmsg * attr.mq_msgsize; /* 消息数据 */`
    ///
    /// Linux 3.4 及更早版本：
    /// `bytes = attr.mq_maxmsg * sizeof(struct msg_msg *) + /* 开销 */ attr.mq_maxmsg * attr.mq_msgsize; /* 消息数据 */`
    ///
    /// 其中 attr 是作为第四个参数传递给 mq_open(3) 的 mq_attr 结构，而 msg_msg 和 posix_msg_tree_node 结构是内核内部结构。
    ///
    /// 公式中的“开销”加数考虑了实现所需的开销字节，并确保用户不能创建无限数量的零长度消息（此类消息仍然会消耗一些系统内存用于记账开销）。
    RLIMIT_MSGQUEUE = 12,
    /// 这指定了可以使用 setpriority(2) 或 nice(2) 提升进程 nice 值的上限。
    /// nice 值的实际上限计算为 20 - rlim_cur。因此，此限制的有效范围是从 1（对应于 nice 值 19）到 40（对应于 nice 值 -20）。
    /// 这种不寻常的范围选择是必要的，因为负数不能指定为资源限制值，因为它们通常具有特殊含义。例如，RLIM_INFINITY 通常等同于 -1。
    /// 有关 nice 值的更多详细信息，请参见 sched(7)。
    RLIMIT_NICE = 13,
    /// 这指定了一个比此进程可以打开的最大文件描述符编号大 1 的值。尝试（open(2)、pipe(2)、dup(2) 等）超过此限制会产生错误 EMFILE。
    /// （历史上，此限制在 BSD 上名为 RLIMIT_OFILE。）
    ///
    /// 自 Linux 4.5 起，此限制还定义了一个非特权进程（没有 CAP_SYS_RESOURCE 能力的进程）可以通过 UNIX 域套接字传递给其他进程的“在途”文件描述符的最大数量。
    /// 此限制适用于 sendmsg(2) 系统调用。有关更多详细信息，请参见 unix(7)。
    RLIMIT_NOFILE = 7,
    /// 这是对调用进程的实际用户 ID 的现存进程（或者更准确地说，在 Linux 上是线程）数量的限制。
    /// 只要属于此进程实际用户 ID 的当前进程数大于或等于此限制，fork(2) 就会失败并返回错误 EAGAIN。
    ///
    /// 对于具有 CAP_SYS_ADMIN 或 CAP_SYS_RESOURCE 能力，或者以实际用户 ID 0 运行的进程，不强制执行 RLIMIT_NPROC 限制。
    RLIMIT_NPROC = 6,
    /// 这是对进程常驻集（驻留在 RAM 中的虚拟页数）的限制（以字节为单位）。
    /// 此限制仅在 Linux 2.4.x（x < 30）中有效，并且仅影响指定 MADV_WILLNEED 的 madvise(2) 调用。
    RLIMIT_RSS = 5,
    /// 这指定了可以使用 sched_setscheduler(2) 和 sched_setparam(2) 为此进程设置的实时优先级的上限。
    ///
    /// 有关实时调度策略的更多详细信息，请参见 sched(7)。
    RLIMIT_RTPRIO = 14,
    /// 这是在实时调度策略下调度的进程在不进行阻塞系统调用的情况下可以消耗的 CPU 时间量的限制（以微秒为单位）。
    /// 就此限制而言，每次进程进行阻塞系统调用时，其消耗的 CPU 时间计数将重置为零。
    /// 如果进程继续尝试使用 CPU 但被抢占、其时间片到期或调用 sched_yield(2)，则 CPU 时间计数不会重置。
    ///
    /// 达到软限制后，会向进程发送 SIGXCPU 信号。
    /// 如果进程捕获或忽略此信号并继续消耗 CPU 时间，那么每秒将生成一次 SIGXCPU，直到达到硬限制，此时会向进程发送 SIGKILL 信号。
    ///
    /// 此限制的预期用途是阻止失控的实时进程锁定系统。
    ///
    /// 有关实时调度策略的更多详细信息，请参见 sched(7)。
    RLIMIT_RTTIME = 15,
    /// 这是可以为调用进程的实际用户 ID 排队的信号数量的限制。为检查此限制，标准信号和实时信号都会被计数。
    /// 但是，此限制仅对 sigqueue(3) 强制执行；始终可以使用 kill(2) 对尚未排队到进程的任何信号的一个实例进行排队。
    RLIMIT_SIGPENDING = 11,
    /// 这是进程栈的最大大小，以字节为单位。达到此限制时，会生成 SIGSEGV 信号。
    /// 要处理此信号，进程必须使用备用信号栈（sigaltstack(2)）。
    ///
    /// 自 Linux 2.6.23 起，此限制还决定了用于进程命令行参数和环境变量的空间量；有关详细信息，请参见 execve(2)。
    RLIMIT_STACK = 3,
}

impl RLimitResource {
    pub const MAX: u8 = 15;
}

pub extern "C" fn prlimit64(
    pid: usize,
    resource: isize,
    new_limit: *const RLimit,
    old_limit: *mut RLimit,
) -> SyscallResult {
    let resource: u8 = resource.try_into().map_err(|_| EINVAL)?;
    let resource: RLimitResource = resource.try_into().map_err(|_| EINVAL)?;

    let task = if pid == 0 {
        cur_task()
    } else {
        &get_task_by_tid(pid).ok_or(ESRCH)?
    };

    let rlimits = &task.process.rlimits;

    let old_lim = if new_limit.is_null() {
        rlimits.read()[resource]
    } else {
        let new_limit = copy_one_from_user(new_limit)?;

        if new_limit.rlim_cur > new_limit.rlim_max {
            return EINVAL;
        }

        // TODO: 加入用户机制后这里要鉴权（一般用户不能更改 rlim_max）

        log::trace!("task {} 的 {resource:?} 限制修改为 {new_limit:?}", task.tid);

        replace(&mut rlimits.write()[resource], new_limit)
    };

    if !old_limit.is_null() {
        copy_one_to_user(old_limit, &old_lim)?;
    }

    SyscallResult::success(0)
}

pub extern "C" fn getrlimit(resource: usize, rlimit: *mut RLimit) -> SyscallResult {
    let resource: u8 = resource.try_into().map_err(|_| EINVAL)?;
    let resource: RLimitResource = resource.try_into().map_err(|_| EINVAL)?;

    copy_one_to_user(rlimit, &cur_task().process.rlimits.read()[resource])?;

    SyscallResult::success(0)
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ITimerVal {
    pub interval: TimeVal,
    pub value: TimeVal,
}

impl ITimerVal {
    pub fn from_pair(pair: Option<(usize, Option<NonZero<usize>>)>) -> Self {
        if let Some((value, interval)) = pair {
            // FIXME: 这里其实有一个竞态
            let rest = value.saturating_sub(get_cpu_tick());
            let rest_us = cpu_tick_to_us(rest);
            let interval_us = cpu_tick_to_us(interval.map_or(0, |x| x.get()));

            Self {
                interval: TimeVal::from_us(interval_us),
                value: TimeVal::from_us(rest_us),
            }
        } else {
            Self {
                interval: TimeVal::from_us(0),
                value: TimeVal::from_us(0),
            }
        }
    }
}

#[repr(u8)]
#[derive(TryFromPrimitive, Debug, Clone, Copy)]
#[allow(non_camel_case_types)]
enum ITimerWhich {
    ITIMER_REAL = 0,
    ITIMER_VIRTUAL = 1,
    ITIMER_PROF = 2,
}

pub extern "C" fn setitimer(
    which: isize,
    new_value: *const ITimerVal,
    old_value: *mut ITimerVal,
) -> SyscallResult {
    let which: u8 = which.try_into().map_err(|_| EINVAL)?;
    let which: ITimerWhich = which.try_into().map_err(|_| EINVAL)?;

    let new_value = copy_one_from_user(new_value)?;

    let interval_us = new_value.interval.to_us();
    let value_us = new_value.value.to_us();

    let old = match which {
        ITimerWhich::ITIMER_REAL => {
            let new_value = if value_us == 0 {
                None
            } else {
                let interval_tick = us_to_cpu_tick(interval_us);
                let value_tick = us_to_cpu_tick(value_us) + get_cpu_tick();

                Some((value_tick, NonZero::new(interval_tick)))
            };

            unsafe { set_itimer_real(cur_task(), new_value) }
        }

        other => {
            log::error!("TODO: setitimer({other:?})");
            return EOPNOTSUPP;
        }
    };

    if !old_value.is_null() {
        copy_one_to_user(old_value, &ITimerVal::from_pair(old))?;
    }

    SyscallResult::success(0)
}

pub extern "C" fn getitimer(which: isize, curr_value: *mut ITimerVal) -> SyscallResult {
    let which: u8 = which.try_into().map_err(|_| EINVAL)?;
    let which: ITimerWhich = which.try_into().map_err(|_| EINVAL)?;

    copy_one_to_user(
        curr_value,
        &ITimerVal::from_pair(get_itimer_real(cur_task())),
    )?;
    SyscallResult::success(0)
}
