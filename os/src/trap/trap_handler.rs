use core::{
    arch::{asm, naked_asm},
    mem::offset_of,
    ops::{Range, RangeInclusive},
};

use lazy_static::lazy_static;
use seq_macro::seq;

use crate::{
    basic::timer::reset_timer_interrupt,
    memory::address::VirtAddr,
    multitask::{current_task, task_scheduler::TASK_SCHEDULER},
    trap::{
        page_fault_type::PageFaultType,
        trap::{Exception, Interrupt, Trap},
    },
};

pub fn init_k() {
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("csrw stvec, {0}", in(reg) trap_handler_raw_from_kernel) },
        target_arch = "loongarch64" => unsafe { asm!("csrwr {0}, 0xc",  in(reg) trap_handler_raw_from_kernel) },
    };
}

#[repr(C)]
#[repr(align(16))] // 上下文须压栈，而 RISC-V 64 和 LoongArch 64 都要求栈按 16 字节对齐
struct TrapContext {
    pub registers: [usize; 31], // x1 ~ x31
    pub status: usize,          // sstatus (RISC-V 64) / PRMD (LoongArch 64)
    pub ret_pc: usize,          // 用户态 PC，即返回后执行的第一条指令
}

#[unsafe(no_mangle)]
extern "C" fn trap_handler(context: &mut TrapContext) {
    recover_task_pointer();

    match Trap::new() {
        Trap::Exception(Exception::Syscall) => {
            use crate::syscall;

            const SYSCALL_ID_REG_INDEX: usize = cfg_select! {
                target_arch = "riscv64" => 16, // a7, r17
                target_arch = "loongarch64" => 10, // $a7, $r11
            };
            const SYSCALL_ARGS_REG_INDEX_RANGE: RangeInclusive<usize> = cfg_select! {
                target_arch = "riscv64" => 9..=14, // a0 ~ a5, r10 ~ r15
                target_arch = "loongarch64" => 3..=8, // $a0 ~ $a5, $r4 ~ $r9
            };
            const SYSCALL_RET_REG_INDEX: usize = cfg_select! {
                target_arch = "riscv64" => 9, // a0, r10
                target_arch = "loongarch64" => 3, // $a0, $r4
            };

            context.ret_pc += 4; // 跳过 `ecall` / `break 0` 指令

            let id = context.registers[SYSCALL_ID_REG_INDEX];
            let args: syscall::Arguments = &context.registers[SYSCALL_ARGS_REG_INDEX_RANGE]
                .try_into()
                .unwrap();

            context.registers[SYSCALL_RET_REG_INDEX] = match syscall::TABLE.get(id) {
                Some(Some(func)) => func(args),
                _ => panic!("不支持的系统调用 ID: {id} ({id:#x})"),
            };
        }
        Trap::Interrupt(Interrupt::TimerInterrupt) => {
            reset_timer_interrupt();

            let mut brw = TASK_SCHEDULER.write();
            if let Some(next_task) =
                brw.on_time_interrupt(unsafe { current_task().as_ref_unchecked() })
            {
                drop(brw);
                crate::run_program(next_task);
            }
        }
        Trap::Exception(fault @ (Exception::LoadPageFault | Exception::StorePageFault)) => {
            if is_trap_from_kernel(context) {
                let fixup = 'get_fixup: {
                    for (addr_range, fixup_addr) in FIXUPS.iter() {
                        if addr_range.contains(&context.ret_pc) {
                            break 'get_fixup Some(*fixup_addr);
                        }
                    }
                    None
                };

                if let Some(fixup) = fixup {
                    // TODO：这里其实也要处理用户态缺页问题，不过现在暂时不用担心
                    crate::println!(
                        "{:#x} 处发生 PageFault，但位于 fixup 表内，准备重定向到 {fixup:#x}",
                        context.ret_pc
                    );
                    context.ret_pc = fixup;
                } else {
                    print_trap_context(context);
                    panic!("内核态发生 PageFault！");
                }
            } else {
                let current_task = unsafe { current_task().as_ref_unchecked() };

                let bad_address = get_bad_address();

                let fault_type = match fault {
                    Exception::LoadPageFault => PageFaultType::Read,
                    Exception::StorePageFault => PageFaultType::Write,
                    _ => unreachable!(),
                };

                let handle_result = current_task
                    .memory_space
                    .write()
                    .handle_page_fault(VirtAddr(bad_address), fault_type);

                if handle_result.is_err() {
                    crate::println!("============☆喜报☆============");
                    crate::println!(
                        "用户态程序（PID = {}）发生 PageFault（类型：{:?}）",
                        current_task.pid,
                        fault_type
                    );
                    print_trap_context(context);
                    crate::println!("===============================");

                    todo!();
                }
            }
        }
        other => {
            print_trap_context(context);
            todo!("TRAP: {other:?}");
        }
    }
}

/// 从用户态进入内核态时，寄存器 tp 的值可能已经发生了改变。此函数会从当前栈中重新恢复 tp 的值，令其指向当前任务的 Task。
///
/// # Safety
/// 此函数本身谈不上不安全，但内核态的绝大多数操作都要求 tp 存储到 Task 的指针。
fn recover_task_pointer() {
    let sp;
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("mv {0}, sp", out(reg) sp) },
        target_arch = "loongarch64" => unsafe { asm!("move {0}, $sp", out(reg) sp) },
    }
    let sp = unsafe { crate::basic::stack::TaskKernelStackPtr::from_addr(sp) };
    let task_ptr = unsafe { *((sp.get_high_addr() as *const usize).sub(1)) };
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("mv tp, {0}", in(reg) task_ptr) },
        target_arch = "loongarch64" => unsafe { asm!("move $tp, {0}", in(reg) task_ptr) },
    }
}

fn is_trap_from_kernel(context: &TrapContext) -> bool {
    cfg_select! {
        target_arch = "riscv64" =>     context.status >> 8 & 0b1 == 1,
        target_arch = "loongarch64" => context.status & 0b11 == 0,
    }
}

fn get_bad_address() -> usize {
    cfg_select! {
        target_arch = "riscv64" => {
            let stval: usize;
            unsafe { asm!("csrr {0}, stval", out(reg) stval) };
            stval
        },
        target_arch = "loongarch64" => {
            let badv: usize;
            unsafe { asm!("csrrd {0}, 0x7", out(reg) badv) };
            badv
        },
    }
}

fn print_trap_context(context: &TrapContext) {
    use crate::println;

    println!("====== TRAP CONTEXT ======");
    println!(
        "source privilege: {}",
        if is_trap_from_kernel(context) {
            "K"
        } else {
            "U"
        },
    );
    println!("return PC: {:#x}", context.ret_pc);
    println!("bad address: {:#x}", get_bad_address())
}

lazy_static! {
    static ref FIXUPS: [(Range<usize>, usize); 1] = {
        let suaccess: usize;
        let euaccess: usize;
        let __copy_uaccess_fault: usize;
        unsafe {
            asm!("la {0}, suaccess", out(reg) suaccess);
            asm!("la {0}, euaccess", out(reg) euaccess);
            asm!("la {0}, __copy_uaccess_fault", out(reg) __copy_uaccess_fault);
        }

        [(suaccess..euaccess, __copy_uaccess_fault)]
    };
}

const TRAP_CONTEXT_SIZE: usize = size_of::<TrapContext>();
const REGS_OFFSET: usize = offset_of!(TrapContext, registers);
const STATUS_OFFSET: usize = offset_of!(TrapContext, status);
const RET_PC_OFFSET: usize = offset_of!(TrapContext, ret_pc);

#[rustc_align(4)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
pub extern "C" fn trap_handler_raw_from_user() {
    seq!(N in 3..=31 {
        naked_asm!(
            // 1. 切换到内核栈
            "csrrw sp, sscratch, sp",
            // 2. 为 TrapContext 留出栈空间
            "addi sp, sp, -{TRAP_CONTEXT_SIZE}",
            // 3. 保存寄存器（x1 ~ x31，不含 x2 (sp)——它现在存放的是内核栈的地址）
            "sd x1, {REGS_OFFSET}(sp)",
            #(
                concat!("sd x", N, ", (", N, "-1)*8+{REGS_OFFSET}(sp)"),
            )*
            // 4. 保存 sstatus 和 sepc
            "csrr a0, sstatus",
            "sd a0, {STATUS_OFFSET}(sp)",
            "csrr a1, sepc",
            "sd a1, {RET_PC_OFFSET}(sp)",
            // 5. 保存用户栈的 sp
            "csrr a2, sscratch",
            "sd a2, {REGS_OFFSET}+8(sp)",
            // 6. 将 stvec 改为内核态的版本
            "la t0, trap_handler_raw_from_kernel",
            "csrw stvec, t0",
            // 7. 调用 trap_handler
            "mv a0, sp", // 传参
            "call trap_handler",
            // 8. 将 stvec 改为用户态的版本
            "la t0, trap_handler_raw_from_user",
            "csrw stvec, t0",
            // 9. 恢复用户栈的 sp（到 sscratch 中）
            "ld a2, {REGS_OFFSET}+8(sp)",
            "csrw sscratch, a2",
            // 10. 恢复 sepc 和 sstatus
            "ld a1, {RET_PC_OFFSET}(sp)",
            "csrw sepc, a1",
            "ld a0, {STATUS_OFFSET}(sp)",
            "csrw sstatus, a0",
            // 11. 恢复先前保存的其它寄存器
            "ld x1, {REGS_OFFSET}(sp)",
            #(
                concat!("ld x", N, ", (", N, "-1)*8+{REGS_OFFSET}(sp)"),
            )*
            // 12. 释放栈空间
            "addi sp, sp, {TRAP_CONTEXT_SIZE}",
            // 13. 切换回用户栈
            "csrrw sp, sscratch, sp",
            // 14. 返回用户态（U 特权级）
            "sret",
            TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
            REGS_OFFSET = const REGS_OFFSET,
            STATUS_OFFSET = const STATUS_OFFSET,
            RET_PC_OFFSET = const RET_PC_OFFSET,
        );
    });
}

#[rustc_align(4096)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
pub extern "C" fn trap_handler_raw_from_user() {
    seq!(N in 4..=31 {
        naked_asm!(
            // 1. 切换到内核栈
            "csrwr $sp, {CSR_SAVE0}", // 交换二者的值
            // 2. 为 TrapContext 留出栈空间
            "addi.d $sp, $sp, -{TRAP_CONTEXT_SIZE}",
            // 3. 保存寄存器（$r1 ~ $r31，不含 $r3 ($sp)——它现在存放的是内核栈的地址）
            "st.d $r1, $sp, {REGS_OFFSET}",
            "st.d $r2, $sp, {REGS_OFFSET}+8",
            #(
                concat!("st.d $r", N, ", $sp, (", N, "-1)*8+{REGS_OFFSET}"),
            )*
            // 4. 保存 PRMD 和 ERA
            "csrrd $t0, {CSR_PRMD}",
            "st.d $t0, $sp, {STATUS_OFFSET}",
            "csrrd $t1, {CSR_ERA}",
            "st.d $t1, $sp, {RET_PC_OFFSET}",
            // 5. 保存用户栈的 $sp
            "csrrd $t2, {CSR_SAVE0}",
            "st.d $t2, $sp, {REGS_OFFSET}+2*8",
            // 6. 将 EENTRY 改为内核态的版本
            "la $t0, trap_handler_raw_from_kernel",
            "csrwr $t0, {CSR_EENTRY}",
            // 7. 调用 trap_handler
            "move $a0, $sp", // 传参
            "bl trap_handler",
            // 8. 将 EENTRY 改为用户态的版本
            "la $t0, trap_handler_raw_from_user",
            "csrwr $t0, {CSR_EENTRY}",
            // 9. 恢复用户栈的 $sp（到 CSR_SAVE0 中）
            "ld.d $t2, $sp, {REGS_OFFSET}+2*8",
            "csrwr $t2, {CSR_SAVE0}",
            // 10. 恢复 ERA 和 PRMD
            "ld.d $t1, $sp, {RET_PC_OFFSET}",
            "csrwr $t1, {CSR_ERA}",
            "ld.d $t0, $sp, {STATUS_OFFSET}",
            "csrwr $t0, {CSR_PRMD}",
            // 11. 恢复先前保存的其它寄存器
            "ld.d $r1, $sp, {REGS_OFFSET}",
            "ld.d $r2, $sp, {REGS_OFFSET}+8",
            #(
                concat!("ld.d $r", N, ", $sp, (", N, "-1)*8+{REGS_OFFSET}"),
            )*
            // 12. 释放栈空间
            "addi.d $sp, $sp, {TRAP_CONTEXT_SIZE}",
            // 13. 切换回用户栈
            "csrwr $sp, {CSR_SAVE0}", // 交换二者的值
            // 14. 返回用户态
            "ertn",
            CSR_SAVE0 = const 0x30,
            CSR_PRMD = const 0x1,
            CSR_ERA = const 0x6,
            CSR_EENTRY = const 0xc,
            TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
            REGS_OFFSET = const REGS_OFFSET,
            STATUS_OFFSET = const STATUS_OFFSET,
            RET_PC_OFFSET = const RET_PC_OFFSET,
        );
    });
}

#[rustc_align(4)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
pub extern "C" fn trap_handler_raw_from_kernel() {
    seq!(N in 3..=31 {
        naked_asm!(
            // 1. 为 TrapContext 留出栈空间
            "addi sp, sp, -{TRAP_CONTEXT_SIZE}",
            // 2. 保存寄存器（x1 ~ x31，不含 x2 (sp)——我们直接在原内核栈上操作，类似于直接函数调用）
            "sd x1, {REGS_OFFSET}(sp)",
            #(
                concat!("sd x", N, ", (", N, "-1)*8+{REGS_OFFSET}(sp)"),
            )*
            // 3. 保存 sstatus 和 sepc
            "csrr a0, sstatus",
            "sd a0, {STATUS_OFFSET}(sp)",
            "csrr a1, sepc",
            "sd a1, {RET_PC_OFFSET}(sp)",
            // 4. 调用 trap_handler
            "mv a0, sp", // 传参
            "call trap_handler",
            // 5. 恢复 sepc 和 sstatus
            "ld a1, {RET_PC_OFFSET}(sp)",
            "csrw sepc, a1",
            "ld a0, {STATUS_OFFSET}(sp)",
            "csrw sstatus, a0",
            // 6. 恢复先前保存的其它寄存器
            "ld x1, {REGS_OFFSET}(sp)",
            #(
                concat!("ld x", N, ", (", N, "-1)*8+{REGS_OFFSET}(sp)"),
            )*
            // 7. 释放栈空间
            "addi sp, sp, {TRAP_CONTEXT_SIZE}",
            // 8. 返回，特权级保持不变
            "sret",
            TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
            REGS_OFFSET = const REGS_OFFSET,
            STATUS_OFFSET = const STATUS_OFFSET,
            RET_PC_OFFSET = const RET_PC_OFFSET,
        );
    });
}

#[rustc_align(4096)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
pub extern "C" fn trap_handler_raw_from_kernel() {
    seq!(N in 4..=31 {
        naked_asm!(
            // 1. 为 TrapContext 留出栈空间
            "addi.d $sp, $sp, -{TRAP_CONTEXT_SIZE}",
            // 2. 保存寄存器（$r1 ~ $r31，不含 $r3 ($sp)——我们直接在原内核栈上操作，类似于直接函数调用）
            "st.d $r1, $sp, {REGS_OFFSET}",
            "st.d $r2, $sp, {REGS_OFFSET}+8",
            #(
                concat!("st.d $r", N, ", $sp, (", N, "-1)*8+{REGS_OFFSET}"),
            )*
            // 3. 保存 PRMD 和 ERA
            "csrrd $t0, {CSR_PRMD}",
            "st.d $t0, $sp, {STATUS_OFFSET}",
            "csrrd $t1, {CSR_ERA}",
            "st.d $t1, $sp, {RET_PC_OFFSET}",
            // 4. 调用 trap_handler
            "move $a0, $sp", // 传参
            "bl trap_handler",
            // 5. 恢复 ERA 和 PRMD
            "ld.d $t1, $sp, {RET_PC_OFFSET}",
            "csrwr $t1, {CSR_ERA}",
            "ld.d $t0, $sp, {STATUS_OFFSET}",
            "csrwr $t0, {CSR_PRMD}",
            // 6. 恢复先前保存的其它寄存器
            "ld.d $r1, $sp, {REGS_OFFSET}",
            "ld.d $r2, $sp, {REGS_OFFSET}+8",
            #(
                concat!("ld.d $r", N, ", $sp, (", N, "-1)*8+{REGS_OFFSET}"),
            )*
            // 7. 释放栈空间
            "addi.d $sp, $sp, {TRAP_CONTEXT_SIZE}",
            // 8. 返回内核态
            "ertn",
            CSR_PRMD = const 0x1,
            CSR_ERA = const 0x6,
            TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
            REGS_OFFSET = const REGS_OFFSET,
            STATUS_OFFSET = const STATUS_OFFSET,
            RET_PC_OFFSET = const RET_PC_OFFSET,
        );
    });
}
