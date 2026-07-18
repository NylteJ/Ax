use core::{arch::asm, ops::Range};

use lazy_static::lazy_static;

use crate::{
    arch::{
        self, NativeTrapContext,
        interfaces::{Regs, Trap as Trap2, TrapContext},
    },
    basic::{
        DropGuard, abort_and_shutdown,
        debugger::debugbreak,
        stack::TaskKernelStackPtr,
        timer::{check_all_timers, reset_timer_interrupt},
    },
    memory::address::VirtAddr,
    multitask::{
        cur_task, current_task, signal::SignalId, task::Task, task_scheduler::TASK_SCHEDULER,
    },
    syscall::{self, ERRORCODE_NOSYS, ERRORCODE_SPECIAL_RESTART_SYSCALL},
    trap::{
        page_fault_type::PageFaultType,
        trap::{Exception, Interrupt, Trap},
    },
};

#[cfg(debug_assertions)]
lazy_static! {
    pub static ref UNKNOWN_SYSCALLS: crate::basic::sync::Mutex<hashbrown::HashMap<usize, usize>> =
        crate::basic::sync::Mutex::new(hashbrown::HashMap::new());
}

#[unsafe(no_mangle)]
extern "C" fn trap_handler(context: &mut NativeTrapContext) {
    let trap_is_from_kernel = context.is_trap_from_kernel();

    let kernel_time_updater;
    let signal_handling_guard;
    if !trap_is_from_kernel {
        cur_task().time_stats.lock().update_user_time();
        kernel_time_updater = DropGuard::new(|| cur_task().time_stats.lock().update_kernel_time());

        signal_handling_guard = DropGuard::new(|| {
            while let Some((signal_id, rt_data)) = cur_task().try_consume_signal() {
                log::debug!(
                    "正在处理 TID = {} 的 Task 的 {signal_id:?} 信号",
                    cur_task().tid
                );

                let behavior = cur_task().signal_handler.read().handle(signal_id, rt_data);

                use crate::multitask::signal::SignalBehavior;
                match behavior {
                    // 这里直接调用 exit_group_kernel 是没问题的，其内部有防重入设施，必要时会退化为 exit
                    SignalBehavior::Terminate => syscall::process::exit_group_kernel(0, signal_id),
                    SignalBehavior::Ignored => (),
                    SignalBehavior::TerminateAndDumpCore => {
                        log::error!("TODO: core dump");
                        syscall::process::exit_group_kernel(0, signal_id);
                    }

                    SignalBehavior::Stop => todo!(),
                    SignalBehavior::ContinueIfStopped => todo!(),
                }
            }
        });
    }

    match Trap::new() {
        Trap::Exception(Exception::Syscall) => {
            use crate::syscall;

            context.ret_pc += 4; // 跳过 `ecall` / `break 0` 指令

            let id = context.syscall_id();
            let args = context.syscall_args();

            let ret_value = match syscall::TABLE.get(id) {
                Some(Some(func)) => func.call(args) as usize,
                _ => ERRORCODE_NOSYS,
            };

            if ret_value == ERRORCODE_NOSYS {
                log::warn!(
                    "未知系统调用：id = {id} ({}), args = {args:x?} (HEX)",
                    syscall::SYSCALL_ID_NAME_MAP.get(id).unwrap_or(&"???")
                );

                #[cfg(debug_assertions)]
                {
                    *UNKNOWN_SYSCALLS.lock().entry(id).or_insert(0) += 1;
                }
            } else {
                log::trace!(
                    "系统调用：TID = {}, id = {id} ({}), args = {args:x?} (HEX), return value = {ret_value:#x}",
                    cur_task().tid,
                    syscall::SYSCALL_ID_NAME_MAP.get(id).unwrap_or(&"???")
                );
            }

            debug_assert!(ret_value != crate::syscall::err_codes::E_SPECIAL_NET_RETRY.raw());

            if ret_value == ERRORCODE_SPECIAL_RESTART_SYSCALL {
                log::trace!(
                    "系统调用 {id} ({}) 请求重启",
                    syscall::SYSCALL_ID_NAME_MAP.get(id).unwrap_or(&"???")
                );
                context.ret_pc -= 4;
                // 这样就够了，应用程序返回时会使用相同的参数再度触发一次系统调用
            } else {
                // 只在不重启时修改返回值，因为许多平台上返回值和参数共用寄存器
                *context.ret_reg_mut() = ret_value;
            }
        }
        Trap::Interrupt(Interrupt::TimerInterrupt) => {
            reset_timer_interrupt();

            check_all_timers();

            let mut brw = TASK_SCHEDULER.lock();
            if let Some(next_task) = brw.on_time_interrupt(cur_task()) {
                drop(brw);
                Task::switch_to(next_task);
            }
        }
        Trap::Exception(fault) if fault.is_page_fault() => {
            // log::trace!("fault: {fault:?}, bad address: {:#x}", arch::NativeTrap::get_bad_address());
            if trap_is_from_kernel {
                let fixup = FIXUPS.iter().find_map(|(addr_range, fixup_addr)| {
                    if addr_range.contains(&context.ret_pc) {
                        Some(*fixup_addr)
                    } else {
                        None
                    }
                });

                if let Some(fixup) = fixup {
                    // 注意这里也是需要处理用户态缺页的
                    if try_handle_user_page_fault(fault).is_ok() {
                        // 处理了，说明不是真的错误，只是普通缺页，返回即可
                        return;
                    }

                    log::info!(
                        "{:#x} 处发生 PageFault，但位于 fixup 表内，准备重定向到 {fixup:#x}",
                        context.ret_pc
                    );
                    context.ret_pc = fixup;
                } else {
                    kernel_fault(context);
                }
            } else {
                if try_handle_user_page_fault(fault).is_err() {
                    user_fault(context);
                }
            }
        }

        Trap::Exception(Exception::IllegalInstruction) => {
            if trap_is_from_kernel {
                kernel_fault(context);
            } else {
                user_fault(context);
            }
        }

        #[cfg(target_arch = "loongarch64")]
        Trap::Exception(Exception::FloatingPointUnavailable) => {
            debug_assert!(!trap_is_from_kernel, "内核不应使用浮点指令");

            log::info!(
                "当前任务（TID = {}）使用了浮点指令！我们的内核不使用浮点寄存器，因此浮点寄存器只会在任务切换时被保存和恢复",
                cur_task().tid
            );

            use bit_field::BitField;

            let mut euen: usize;
            unsafe { asm!("csrrd {0}, 0x2", out(reg) euen) };
            euen.set_bit(0, true);
            unsafe { asm!("csrwr {0}, 0x2", inlateout(reg) euen => _) };
        }

        #[cfg(target_arch = "loongarch64")]
        Trap::Exception(Exception::PagePrivilegeIllegal) => {
            log::error!("TODO：调查原因");
            cur_task().send_signal(SignalId::SIGSEGV);
        }

        other => {
            print_trap_context(context);
            todo!("TRAP: {other:?}");
        }
    }
}

fn try_handle_user_page_fault(fault: Exception) -> Result<(), ()> {
    let bad_address = arch::NativeTrap::get_bad_address();

    let fault_type = match fault {
        Exception::LoadPageFault => PageFaultType::Read,
        Exception::StorePageFault => PageFaultType::Write,
        Exception::InstructionPageFault => PageFaultType::Execute,
        #[cfg(target_arch = "loongarch64")]
        Exception::PageModifyFault => PageFaultType::Write,
        #[cfg(target_arch = "loongarch64")]
        Exception::PageNonReadableFault => PageFaultType::Read,
        #[cfg(target_arch = "loongarch64")]
        Exception::PageNonExecutableFault => PageFaultType::Execute,
        _ => unreachable!(),
    };

    cur_task()
        .address_space
        .read()
        .write()
        .handle_page_fault(VirtAddr(bad_address), fault_type)
}

fn print_trap_context(context: &NativeTrapContext) {
    use crate::println;

    println!("====== TRAP CONTEXT ======");
    println!(
        "source privilege: {}",
        if context.is_trap_from_kernel() {
            "K"
        } else {
            "U"
        },
    );
    println!("return PC: {:#x}", context.ret_pc);
    println!("bad address: {:#x}", arch::NativeTrap::get_bad_address());

    if let Some(task) = unsafe { current_task().as_ref() } {
        println!("Task TID: {}", task.tid);
    }

    if context.is_trap_from_kernel() {
        println!("异常发生地点的调用堆栈：");
        crate::basic::debugger::backtrace_at(context.regs.fp as *const usize);
    }
}

/// 从用户态进入内核态时，寄存器 tp 的值可能已经发生了改变。此函数会从当前栈中重新恢复 tp 的值，令其指向当前任务的 Task。
///
/// # Safety
/// 此函数本身谈不上不安全，但内核态的绝大多数操作都要求 tp 存储到 Task 的指针。
pub extern "C" fn recover_task_pointer() {
    let sp = arch::NativeRegs::get_sp();
    let sp = unsafe { TaskKernelStackPtr::from_addr(sp) };

    let task_addr = unsafe { *((sp.get_high_addr() as *const usize).sub(1)) };

    unsafe { Task::set_current_task(task_addr as *const Task) };
}

lazy_static! {
    static ref FIXUPS: [(Range<usize>, usize); 3] = {
        macro_rules! define {
            ($ident:ident) => {
                {
                    let start: usize;
                    let end: usize;
                    let fault: usize;
                    unsafe {
                        asm!(concat!("la {0}, suaccess_", stringify!($ident)), out(reg) start);
                        asm!(concat!("la {0}, euaccess_", stringify!($ident)), out(reg) end);
                        asm!(concat!("la {0}, __", stringify!($ident), "_uaccess_fault"), out(reg) fault);
                    }

                    (start..end, fault)
                }
            };
        }

        [define!(copy), define!(strcpy), define!(memset)]
    };
}

#[cold]
#[inline(never)]
fn kernel_fault(context: &NativeTrapContext) -> ! {
    use crate::{print, println};

    println!("内核态发生 PageFault！");
    print_trap_context(context);

    println!("============其它上下文信息============");
    print!("返回 PC：{:#x}  ", context.ret_pc);
    crate::basic::debugger::print_pc_info_if_debug(context.ret_pc);
    println!();

    println!("栈帧指针：{:#x}", context.regs.fp);
    crate::basic::debugger::backtrace_at(context.regs.fp as *const usize);
    println!();

    println!("寄存器信息：\n{:#?}", context.regs);

    debugbreak();

    abort_and_shutdown();
}

fn user_fault(context: &NativeTrapContext) {
    log::error!(
        "=====================================☆\u{fe0f}喜报☆\u{fe0f}====================================="
    );
    log::error!("用户态程序（PID = {}）发生 Fault", cur_task().tid);
    log::info!("其试图访问：{:#x}", arch::NativeTrap::get_bad_address());
    log::info!("对应代码所在地址：{:#x}", context.ret_pc);
    log::debug!("寄存器信息：\n{:#?}", context.regs);
    log::error!(
        "=================================================================================="
    );

    let sigsegv_blocked = cur_task()
        .signals_wl
        .lock()
        .mask
        .contains(SignalId::SIGSEGV.as_mask());
    if sigsegv_blocked {
        log::error!("然而，其屏蔽了 SIGSEGV");
        log::error!("为避免无限递归，内核将强行终止程序");
        cur_task().send_signal(SignalId::SIGKILL);
    } else {
        cur_task().send_signal(SignalId::SIGSEGV);
    }
}
