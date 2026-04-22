use core::{
    arch::{asm, naked_asm},
    mem::offset_of,
    ops::RangeInclusive,
};

use seq_macro::seq;

use crate::{
    basic::timer::reset_timer_interrupt,
    trap::trap::{Exception, Interrupt, Trap},
};

pub fn init() {
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("csrw stvec, {0}", in(reg) trap_handler_raw) },
        target_arch = "loongarch64" => unsafe { asm!("csrwr {0}, 0xc",  in(reg) trap_handler_raw) },
    };
}

#[repr(C)]
#[repr(align(16))] // 上下文须压栈，而 RISC-V 64 和 LoongArch 64 都要求栈按 16 字节对齐
struct TrapContext {
    pub registers: [usize; 31], // x1 ~ x31
    pub sstatus: usize,         // OR PRMD (LoongArch64)
    pub ret_pc: usize,          // 用户态 PC，即返回后执行的第一条指令
}

#[unsafe(no_mangle)]
pub extern "C" fn trap_handler(context: &mut TrapContext) {
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

            let cur = crate::multitask::current_task();
            crate::multitask::task_scheduler::TASK_SCHEDULER
                .write()
                .add_task(unsafe {
                    core::pin::Pin::new_unchecked(alloc::sync::Arc::from_raw(cur))
                });
            crate::run_next_program();
        }
        other => todo!("TRAP: {other:?}"),
    }
}

/// 从用户态进入内核态时，寄存器 tp 的值可能已经发生了改变。此函数会从当前栈中重新恢复 tp 的值，令其指向当前任务的 Task。
///
/// 注意：此函数本身谈不上不安全，但内核态的绝大多数操作都要求 tp 存储到 Task 的指针。
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

#[rustc_align(4)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv64")]
pub extern "C" fn trap_handler_raw() {
    static_assertions::const_assert_eq!(size_of::<TrapContext>(), 34 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, registers), 0);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, sstatus), 31 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, ret_pc), 32 * 8);

    seq!(N in 3..=31 {
        naked_asm!(
            // 1. 切换到内核栈
            "csrrw sp, sscratch, sp",
            // 2. 为 TrapContext 留出栈空间
            "addi sp, sp, -34*8",
            // 3. 保存寄存器（x1 ~ x31，不含 x2 (sp)——它现在存放的是内核栈的地址）
            "sd x1, 0(sp)",
            #(
                concat!("sd x", N, ", (", N, "-1)*8(sp)"),
            )*
            // 4. 保存 sstatus 和 sepc
            "csrr a0, sstatus",
            "sd a0, 31*8(sp)",
            "csrr a1, sepc",
            "sd a1, 32*8(sp)",
            // 5. 保存用户栈的 sp
            "csrr a2, sscratch",
            "sd a2, 1*8(sp)",
            // 6. 调用 trap_handler
            "mv a0, sp", // 传参
            "call trap_handler",
            // 7. 恢复用户栈的 sp（到 sscratch 中）
            "ld a2, 1*8(sp)",
            "csrw sscratch, a2",
            // 8. 恢复 sepc 和 sstatus
            "ld a1, 32*8(sp)",
            "csrw sepc, a1",
            "ld a0, 31*8(sp)",
            "csrw sstatus, a0",
            // 9. 恢复先前保存的其它寄存器
            "ld x1, 0(sp)",
            #(
                concat!("ld x", N, ", (", N, "-1)*8(sp)"),
            )*
            // 10. 释放栈空间
            "addi sp, sp, 34*8",
            // 11. 切换回用户栈
            "csrrw sp, sscratch, sp",
            // 12. 返回用户态（U 特权级）
            "sret",
        );
    });
}

#[rustc_align(4096)]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[cfg(target_arch = "loongarch64")]
pub extern "C" fn trap_handler_raw() {
    static_assertions::const_assert_eq!(size_of::<TrapContext>(), 34 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, registers), 0);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, sstatus), 31 * 8);
    static_assertions::const_assert_eq!(offset_of!(TrapContext, ret_pc), 32 * 8);

    seq!(N in 4..=31 {
        naked_asm!(
            // 1. 切换到内核栈
            "csrwr $sp, {CSR_SAVE0}", // 交换二者的值
            // 2. 为 TrapContext 留出栈空间
            "addi.d $sp, $sp, -34*8",
            // 3. 保存寄存器（$r1 ~ $r31，不含 $r3 ($sp)——它现在存放的是内核栈的地址）
            "st.d $r1, $sp, 0*8",
            "st.d $r2, $sp, 1*8",
            #(
                concat!("st.d $r", N, ", $sp, (", N, "-1)*8"),
            )*
            // 4. 保存 PRMD 和 ERA
            "csrrd $t0, {CSR_PRMD}",
            "st.d $t0, $sp, 31*8",
            "csrrd $t1, {CSR_ERA}",
            "st.d $t1, $sp, 32*8",
            // 5. 保存用户栈的 $sp
            "csrrd $t2, {CSR_SAVE0}",
            "st.d $t2, $sp, 2*8",
            // 6. 调用 trap_handler
            "move $a0, $sp", // 传参
            "bl trap_handler",
            // 7. 恢复用户栈的 $sp（到 CSR_SAVE0 中）
            "ld.d $t2, $sp, 2*8",
            "csrwr $t2, {CSR_SAVE0}",
            // 8. 恢复 sepc 和 sstatus
            "ld.d $t1, $sp, 32*8",
            "csrwr $t1, {CSR_ERA}",
            "ld.d $t0, $sp, 31*8",
            "csrwr $t0, {CSR_PRMD}",
            // 9. 恢复先前保存的其它寄存器
            "ld.d $r1, $sp, 0*8",
            "ld.d $r2, $sp, 1*8",
            #(
                concat!("ld.d $r", N, ", $sp, (", N, "-1)*8"),
            )*
            // 10. 释放栈空间
            "addi.d $sp, $sp, 34*8",
            // 11. 切换回用户栈
            "csrwr $sp, {CSR_SAVE0}", // 交换二者的值
            // 12. 返回用户态
            "ertn",
            CSR_SAVE0 = const 0x30,
            CSR_PRMD = const 0x1,
            CSR_ERA = const 0x6,
        );
    });
}
