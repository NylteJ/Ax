use core::ffi::c_int;
use core::ffi::c_void;
use core::mem::offset_of;
use core::mem::replace;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use bitflags::Flags;
use bitflags::bitflags;
use derive_more::Debug;
use num_enum::TryFromPrimitive;

use crate::arch;
use crate::basic::timer::get_cpu_tick;
use crate::basic::timer::us_to_cpu_tick;
use crate::memory::uaccess::copy_one_from_user;
use crate::memory::uaccess::copy_one_to_user;
use crate::multitask::cur_task;
use crate::multitask::signal::SignalHandlerType;
use crate::multitask::signal::SignalId;
use crate::multitask::signal::SignalIdUnderlying;
use crate::multitask::signal::SignalMask;
use crate::multitask::signal::sigreturn;
use crate::multitask::task::Task;
use crate::multitask::task_manager::foreach_task;
use crate::multitask::task_manager::get_task_by_tid;
use crate::multitask::wait::SleepError;
use crate::multitask::wait::Waiting;
use crate::syscall::SyscallResult;

use crate::syscall::common::TimeSpec;
use crate::syscall::common::get_timeout_us;
use crate::syscall::err_codes::*;

pub fn restart_syscall_if_able(sleep_err: SleepError) -> SyscallResult {
    let SleepError::Interrupted(signal) = sleep_err;

    if cur_task().signal_handler.read().handlers[signal as usize]
        .sa_flags
        .contains(SignalActionFlags::SA_RESTART)
    {
        return E_SPECIAL_RESTART_SYSCALL;
    } else {
        return EINTR;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalAction {
    // 注意此处布局和 https://www.man7.org/linux/man-pages/man2/sigaction.2.html 上的不一致
    #[debug("{:#x}", self.handler.addr())]
    pub handler: SignalActionHandler,
    pub sa_flags: SignalActionFlags,
    pub sa_restorer: Option<extern "C" fn()>,
    pub sa_mask: SignalMask,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union SignalActionHandler {
    pub sa_handler: Option<extern "C" fn(c_int)>,
    pub sa_sigaction: Option<extern "C" fn(c_int, *mut SignalInfo, *mut c_void)>,
}

impl SignalActionHandler {
    pub fn addr(&self) -> usize {
        unsafe {
            match *self {
                Self { sa_handler } => sa_handler.map_or(0, |f| f as usize),
                Self { sa_sigaction } => sa_sigaction.map_or(0, |f| f as usize),
            }
        }
    }
}

impl SignalAction {
    pub fn get_handler_type(&self) -> SignalHandlerType {
        let addr = self.handler.addr();
        match addr {
            0 => SignalHandlerType::Default,
            1 => SignalHandlerType::Ignored,
            _ => {
                let ptr = NonNull::new(addr as *mut ()).unwrap();

                if self.sa_flags.contains(SignalActionFlags::SA_SIGINFO) {
                    SignalHandlerType::UserDefinedDetailed(ptr)
                } else {
                    SignalHandlerType::UserDefinedSimple(ptr)
                }
            }
        }
    }

    pub fn is_custom(&self) -> bool {
        self.handler.addr() > 1
    }
}

impl Default for SignalAction {
    fn default() -> Self {
        Self {
            handler: SignalActionHandler { sa_handler: None },
            sa_mask: SignalMask::empty(),
            sa_flags: SignalActionFlags::empty(),
            sa_restorer: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignalInfo {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    /// 其它字段随信号不同可能变化
    pub rest: [i32; 29],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UContext {
    pub flags: usize,
    pub link: *mut UContext,
    pub ss_sp: *mut c_void,
    pub ss_flags: c_int,
    pub ss_size: usize,
    pub mask: SignalMask,
    /// 我不知道这个偏移是哪里来的，源代码里根本找不到
    /// 但反汇编说 ret_pc 应该放在 0xb0，那就放在 0xb0 吧
    /// （为此 LoongArch 64 的自带 __pad 都去掉了）
    pub __pad: [u8; 0xb0 - 0x30],
    pub mcontext: arch::NativeMContext,
}
static_assertions::const_assert_eq!(offset_of!(UContext, mask), 40);
static_assertions::const_assert_eq!(offset_of!(UContext, mcontext), 0xb0);

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy)]
    pub struct SignalActionFlags: c_int {
        // https://www.man7.org/linux/man-pages/man2/sigaction.2.html

        /// 如果 signum 是 SIGCHLD，则在子进程停止（即，当它们收到 SIGSTOP、SIGTSTP、SIGTTIN 或 SIGTTOU 中的某一个时）
        /// 或恢复执行（即，它们收到 SIGCONT）时，不会收到通知（参见 wait(2)）。此标志仅在为 SIGCHLD 建立处理器时才有意义。
        const SA_NOCLDSTOP = 1 << 0;
        /// 如果 signum 是 SIGCHLD，则在子进程终止时不要将其转化为僵尸进程。另请参见 waitpid(2)。
        /// 此标志仅在为 SIGCHLD 建立处理器，或将该信号的处理动作设置为 SIG_DFL 时才有意义。
        ///
        /// 如果在为 SIGCHLD 建立处理器时设置了 SA_NOCLDWAIT 标志，当子进程终止时是否会产生 SIGCHLD 信号，POSIX.1 并未明确规定。
        /// 在 Linux 上，这种情况下会产生 SIGCHLD 信号；在其他某些实现中，则不会。
        const SA_NOCLDWAIT = 1 << 1;
        /// 信号处理器接受三个参数，而不是一个。在这种情况下，应设置 sa_sigaction 而不是 sa_handler。
        /// 此标志仅在建立信号处理器时才有意义。
        const SA_SIGINFO = 1 << 2;
        /// 不供应用程序使用。此标志被 C 库用于指示 sa_restorer 字段包含一个“信号跳板”的地址。更多细节参见 sigreturn(2)。
        const SA_RESTORER = 1 << 26;
        /// 在由 sigaltstack(2) 提供的备用信号栈上调用信号处理器。如果没有可用的备用栈，则使用默认栈。
        /// 此标志仅在建立信号处理器时才有意义。
        const SA_ONSTACK = 1 << 27;
        /// 通过使某些系统调用在信号间可重启，提供与 BSD 信号语义兼容的行为。此标志仅在建立信号处理器时才有意义。
        /// 关于系统调用重启的讨论，参见 signal(7)。
        const SA_RESTART = 1 << 28;
        /// 在执行信号处理器期间，不要将该信号添加到线程的信号掩码中，除非该信号在 act.sa_mask 中被指定。
        /// 因此，当线程在执行处理器时，可能会向它递送该信号的另一个实例。此标志仅在建立信号处理器时才有意义。
        const SA_NODEFER = 1 << 30;
        /// 在进入信号处理器时，将信号的动作恢复为默认动作。此标志仅在建立信号处理器时才有意义。
        const SA_RESETHAND = 1 << 31;
    }
}

pub extern "C" fn rt_sigaction(
    signum: isize,
    act: *const SignalAction,
    oldact: *mut SignalAction,
) -> SyscallResult {
    let signum: SignalIdUnderlying = signum.try_into().map_err(|_| EINVAL)?;
    let sig_id: SignalId = signum.try_into().map_err(|_| EINVAL)?;
    if sig_id.is_special() || sig_id == SignalId::SIGINVAL {
        return EINVAL;
    }

    let old_act = if act.is_null() {
        cur_task().signal_handler.read().handlers[sig_id as usize]
    } else {
        let mut act = copy_one_from_user(act)?;

        if act.sa_flags.contains_unknown_bits() || act.sa_mask.contains_unknown_bits() {
            return EINVAL;
        }

        // 无法屏蔽这类信号，它们会被静默忽略
        act.sa_mask.remove_special();

        log::trace!(
            "TID = {} 的 Task 设置信号 {sig_id:?} 的处理方式为 {act:?}",
            cur_task().tid
        );

        replace(
            &mut cur_task().signal_handler.write().handlers[sig_id as usize],
            act,
        )
    };

    if !oldact.is_null() {
        copy_one_to_user(oldact, &old_act)?;
    }

    SyscallResult::success(0)
}

pub extern "C" fn kill(pid: isize, sig: isize) -> SyscallResult {
    let sig: SignalIdUnderlying = sig.try_into().map_err(|_| EINVAL)?;
    let signal: SignalId = sig.try_into().map_err(|_| EINVAL)?;

    // TODO: 鉴权

    enum Target {
        Pid(usize),
        Pgid(usize),
        All,
    }

    let target = match pid {
        ..-1 => Target::Pgid((-pid).cast_unsigned()),
        -1 => Target::All,
        0 => Target::Pgid(cur_task().process.pgid.load(Ordering::Relaxed)),
        1.. => Target::Pid(pid.cast_unsigned()),
    };

    let send_signal_if_not_siginval = |task: &Task| {
        if signal != SignalId::SIGINVAL {
            task.process.send_signal(signal);
        }
        // 否则，不实际发送任何信号，只执行完整的错误检查
    };

    match target {
        Target::Pid(pid) => {
            let task = get_task_by_tid(pid).ok_or(ESRCH)?;

            if task.process.pid != task.tid {
                return ESRCH;
            }

            send_signal_if_not_siginval(&task);
        }
        Target::Pgid(pgid) => {
            // FIXME: 现在这个实现的性能只能用悲剧形容
            let mut any = false;

            foreach_task(|task| {
                if task.process.pgid.load(Ordering::Relaxed) == pgid {
                    send_signal_if_not_siginval(task);
                    any = true;
                }
            });

            if !any {
                return ESRCH;
            }
        }
        Target::All => todo!(),
    }

    SyscallResult::success(0)
}

pub extern "C" fn tgkill(tgid: isize, tid: isize, sig: isize) -> SyscallResult {
    let sig: SignalIdUnderlying = sig.try_into().map_err(|_| EINVAL)?;
    let signal: SignalId = sig.try_into().map_err(|_| EINVAL)?;

    get_task_by_tid(tid.cast_unsigned())
        .and_then(|task| {
            if tgid == -1 || task.process.pid == tgid.cast_unsigned() {
                Some(task)
            } else {
                None
            }
        })
        .ok_or(ESRCH)?
        .send_signal(signal);

    SyscallResult::success(0)
}

pub extern "C" fn tkill(tid: isize, sig: isize) -> SyscallResult {
    tgkill(-1, tid, sig)
}

pub extern "C" fn rt_sigprocmask(
    how: isize,
    set: *const SignalMask,
    oldset: *mut SignalMask,
) -> SyscallResult {
    let old_set = {
        let mut signals_wl_guard = cur_task().signals_wl.lock();
        let mask = &mut signals_wl_guard.mask;
        let old_mask = *mask;

        if !set.is_null() {
            #[derive(TryFromPrimitive)]
            #[repr(u8)]
            enum How {
                Block = 0,
                Unblock = 1,
                SetMask = 2,
            }

            let how: u8 = how.try_into().map_err(|_| EINVAL)?;
            let how: How = how.try_into().map_err(|_| EINVAL)?;

            let set = copy_one_from_user(set)?;
            if set.contains_unknown_bits() {
                return EINVAL;
            }
            match how {
                How::Block => mask.insert(set),
                How::Unblock => mask.remove(set),
                How::SetMask => *mask = set,
            }

            mask.remove_special();
        }

        old_mask
    };

    if !oldset.is_null() {
        copy_one_to_user(oldset, &old_set)?;
    }

    SyscallResult::success(0)
}

pub extern "C" fn rt_sigtimedwait(
    set: *const SignalMask,
    info: *mut SignalInfo,
    timeout: *const TimeSpec,
) -> SyscallResult {
    // 等待 SIGKILL 和 SIGSTOP 的请求会被静默忽略
    let unwaitable_signals = SignalMask::SIGKILL | SignalMask::SIGSTOP;

    let wait = copy_one_from_user(set)?.difference(unwaitable_signals);
    let mask = !wait;

    let timeout_us = get_timeout_us(timeout)?;
    let deadline_tick = timeout_us.map(|us| get_cpu_tick() + us_to_cpu_tick(us as usize));

    loop {
        let consumed = cur_task()
            .signals_wl
            .lock()
            .try_consume_one_with_mask(mask)
            .or_else(|| {
                cur_task()
                    .process
                    .signals_wl
                    .lock()
                    .try_consume_one_with_mask(mask)
            });

        if let Some((signal_id, _)) = consumed {
            log::trace!("Task {} 等待到了信号 {signal_id:?}", cur_task().tid);
            return SyscallResult::success(signal_id as usize);
        } else if cur_task().find_signal_to_consume().is_some() {
            // 被未阻塞的信号打断
            return EINTR;
        }

        if let Some(deadline_tick) = deadline_tick
            && get_cpu_tick() >= deadline_tick
        {
            return EAGAIN;
        }

        crate::syscall::basic::yield_(); // 假装睡眠
    }
}

pub extern "C" fn rt_sigreturn() -> usize {
    sigreturn()
}

pub extern "C" fn rt_sigsuspend(mask: *const SignalMask) -> SyscallResult {
    let mut mask = copy_one_from_user(mask)?;
    mask.remove_special();
    let mask = mask;

    let old_mask = replace(&mut cur_task().signals_wl.lock().mask, mask);

    let err = Waiting::new().sleep().expect_err("我们根本没有设定正常唤醒条件");
    Err(err)?
}
