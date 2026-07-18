use core::{
    arch::naked_asm,
    mem::replace,
    ptr::{NonNull, null_mut},
};

use lazy_static::lazy_static;

use crate::{
    arch::{
        self,
        interfaces::{Basic, TrapContext},
    },
    memory::{
        PAGE_BYTES, STACK_ALIGN,
        address::{PhysPageNum, USER_RESERVED_MEMORY_START, VirtAddr},
        page::PTEFlags,
        page_meta::{PAGE_METADATAS, PageType},
        uaccess::{CopyError, copy_one_from_user, copy_one_to_user},
    },
    multitask::{
        cur_task,
        signal::{RtSignalData, SignalBehavior, SignalId},
    },
    syscall::signal::{SignalAction, SignalActionFlags, SignalInfo, UContext},
};

const HANDLERS_COUNT: usize = SignalId::MAX as usize + 1;

#[derive(Debug, Clone)]
pub struct SignalHandler {
    pub handlers: [SignalAction; HANDLERS_COUNT],
}

impl SignalHandler {
    pub fn new() -> Self {
        Self {
            handlers: [SignalAction::default(); HANDLERS_COUNT],
        }
    }

    #[must_use]
    pub fn handle(&self, signal: SignalId, rt_data: Option<RtSignalData>) -> SignalBehavior {
        let action = self.handlers[signal as usize];
        let handler_type = action.get_handler_type();

        let (detailed, fptr) = match handler_type {
            SignalHandlerType::Default => return signal.default_behavior(),
            SignalHandlerType::Ignored => return SignalBehavior::Ignored,
            SignalHandlerType::UserDefinedSimple(fptr) => (false, fptr),
            SignalHandlerType::UserDefinedDetailed(fptr) => (true, fptr),
        };

        let call_user_handler = || -> Result<(), CopyError> {
            // 调用用户自定义的处理函数
            // 这部分通常是冷路径中的冷路径，因此性能非常无所谓

            let trap_context =
                unsafe { cur_task().get_trap_context().cast_mut().as_mut_unchecked() };

            let mcontext = trap_context.get_mcontext();

            let old_user_sp = trap_context.regs.sp;
            let mut user_sp = old_user_sp / STACK_ALIGN * STACK_ALIGN;

            // 原子替换 mask，并保存旧 mask
            let old_signal_mask = {
                let mut wl = cur_task().signals_wl.lock();
                replace(&mut wl.mask, action.sa_mask | signal.as_mask())
            };

            // 篡改 TrapContext，让其返回到用户自定义的处理函数，使用新的栈区域并传递对应参数
            // void (*sa_handler)(int);
            // void (*sa_sigaction)(int, siginfo_t *, void *);
            trap_context.regs.a0 = signal.value() as usize;
            if detailed {
                let siginfo = SignalInfo {
                    si_signo: signal.value().into(),
                    si_errno: 0,
                    si_code: 0, // FIXME
                    rest: Default::default(),
                };

                user_sp -= size_of::<SignalInfo>();
                static_assertions::const_assert_eq!(size_of::<SignalInfo>() % STACK_ALIGN, 0);
                debug_assert_eq!(user_sp % STACK_ALIGN, 0);

                let siginfo_ptr = user_sp as *mut SignalInfo;
                copy_one_to_user(siginfo_ptr, &siginfo)?;

                trap_context.regs.a1 = siginfo_ptr.addr();
            }

            debug_assert_eq!(user_sp % STACK_ALIGN, 0);

            // 压入 UContext。无论我们是否需要真正传入它，都执行压入，反正我们确实需要一个结构体保存一些原始状态，而前面也提到了此时的性能并不重要
            // 注意，因为我们在恢复时需要用到其中的值，我们总是将它放在最低地址，这样恢复时 sp 指的就是它的地址
            let ucontext = UContext {
                flags: 0,
                link: null_mut(),
                ss_sp: null_mut(),
                ss_flags: 2, // SS_DISABLE
                ss_size: 0,
                mask: old_signal_mask,
                __pad: [0; 0xb0 - 0x30],
                mcontext,
            };

            user_sp -= size_of::<UContext>();
            static_assertions::const_assert_eq!(size_of::<UContext>() % STACK_ALIGN, 0);
            debug_assert_eq!(user_sp % STACK_ALIGN, 0);

            let ucontext_ptr = user_sp as *mut UContext;
            copy_one_to_user(ucontext_ptr, &ucontext)?;

            if detailed {
                trap_context.regs.a2 = ucontext_ptr.addr();
            }

            trap_context.regs.sp = user_sp;
            trap_context.ret_pc = fptr.as_ptr().addr();

            let trampoline = if action.sa_flags.contains(SignalActionFlags::SA_RESTORER) {
                // 使用用户给定的跳板，哪怕给定的是空指针（万一用户真的在零地址处映射了点东西呢）
                action.sa_restorer.map_or(0, |func| func as usize)
            } else {
                // 设置返回的信号处理跳板（libc 不会调用 sigreturn，所以我们必须在用户态映射里手动添加一个函数）
                // 前面也说了，此时的性能非常不重要，因此我们选择在这里才创建对应映射
                const SIGRET_PAGE_VADDR: VirtAddr =
                    VirtAddr(USER_RESERVED_MEMORY_START).floor_to_page();
                {
                    let addr_space = cur_task().address_space.read();
                    let mut addr_space = addr_space.write();
                    let map_result = addr_space.get_page_table_mut().map::<0>(
                        SIGRET_PAGE_VADDR.into(),
                        *SIGNAL_TRAMPOLINE_PPAGE,
                        PTEFlags::Execute,
                    );
                    if let Err(existing) = map_result {
                        debug_assert_eq!(PhysPageNum::from_pte(existing), *SIGNAL_TRAMPOLINE_PPAGE);
                    } else {
                        drop(addr_space);
                        arch::NativeBasic::tlb_invalidate_addr(SIGRET_PAGE_VADDR);
                    }
                }

                SIGRET_PAGE_VADDR.0 + (signal_return_trampoline as usize % PAGE_BYTES)
            };

            trap_context.regs.ra = trampoline;

            debug_assert_eq!(trap_context.regs.sp, ucontext_ptr.addr());
            Ok(())
        };

        match call_user_handler() {
            Ok(_) => {
                // 信号将交由用户处理，内核方可以忽略它了
                SignalBehavior::Ignored
            }
            Err(err) => {
                log::error!(
                    "处理信号 {signal:?} 时发生故障：无法调用用户定义的信号处理函数，错误 = {err:?}，直接终止进程..."
                );
                // Linux 还会装模做样地发一遍 SIGSEGV，我们这里选择直接终止掉
                SignalBehavior::TerminateAndDumpCore
            }
        }
    }

    /// 将所有自定义处理程序重置为默认值，不修改其它处理程序
    ///
    /// 用于 execve
    pub fn reset_custom_handlers(&mut self) {
        for handler in self.handlers.iter_mut().filter(|act| act.is_custom()) {
            *handler = SignalAction::default();
        }
    }
}

pub enum SignalHandlerType {
    Default,
    Ignored,
    UserDefinedSimple(NonNull<()>),
    UserDefinedDetailed(NonNull<()>),
}

// 这里理论上会泄露一些内核源代码，但没什么影响因为我们是开源的
#[naked]
extern "C" fn signal_return_trampoline() -> ! {
    // 注意这里我们处于用户态，为了防止编译器乱搞，不得不使用 naked_asm

    #[cfg(target_arch = "riscv64")]
    unsafe {
        naked_asm!("li a7, 139", "ecall",);
    }
    #[cfg(target_arch = "loongarch64")]
    unsafe {
        naked_asm!("li.d $a7, 139", "syscall 0",);
    }
}

lazy_static! {
    static ref SIGNAL_TRAMPOLINE_PPAGE: PhysPageNum<0> = {
        let vaddr = VirtAddr(signal_return_trampoline as usize);
        let paddr = vaddr.to_physical_address();
        let ppage = PhysPageNum::from(paddr);

        let guard = unsafe { PAGE_METADATAS.mark_new_page(ppage, PageType::Phantom) };
        debug_assert_eq!(guard.get_page_num(), ppage);

        ppage
    };
}

pub fn sigreturn() -> usize {
    // 注意，此时内核栈顶的那个 TrapContext 是用户主程序的 TrapContext，信号处理程序的上下文在用户栈上
    // 以及，用户程序可以修改上下文，因此我们必须将用户栈上的上下文恢复到主程序的 TrapContext 中
    // 只恢复寄存器（31 个普通寄存器 + 返回 pc）即可，其它值用户不会修改，我们也没有改动
    let main_trap_context = unsafe { cur_task().get_trap_context().cast_mut().as_mut_unchecked() };
    let ucontext_ptr = main_trap_context.regs.sp as *const UContext;
    let ucontext = copy_one_from_user(ucontext_ptr).expect("写入时没出现故障，则读取时也不该出现");

    main_trap_context.with_mcontext(&ucontext.mcontext);

    // 除此之外，还要还原原来的信号掩码
    cur_task().signals_wl.lock().mask = ucontext.mask;

    main_trap_context.ret_reg()
}
