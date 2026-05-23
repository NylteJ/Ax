use core::{
    arch::{asm, naked_asm},
    mem::offset_of,
};

use bit_field::BitField;
use derive_more::Debug;
use seq_macro::seq;

use crate::{arch::interfaces, trap::trap_handler::recover_task_pointer};

pub struct Trap;

unsafe impl interfaces::Trap for Trap {
    type TrapContext = TrapContext;

    #[rustc_align(4)]
    #[unsafe(naked)]
    extern "C" fn trap_handler_raw_from_user() {
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
                "la t0, {trap_handler_raw_from_kernel}",
                "csrw stvec, t0",
                // 7. 恢复 tp
                "call {recover_task_pointer}",
                // 8. 调用 trap_handler
                "mv a0, sp", // 传参
                "call trap_handler",
                // 9. 返回用户态（U 特权级）
                "j {return_to_user}", // 尾调用，无条件跳转即可
                TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
                REGS_OFFSET = const REGS_OFFSET,
                STATUS_OFFSET = const STATUS_OFFSET,
                RET_PC_OFFSET = const RET_PC_OFFSET,
                trap_handler_raw_from_kernel = sym Self::trap_handler_raw_from_kernel,
                recover_task_pointer = sym recover_task_pointer,
                return_to_user = sym Self::return_to_user,
            );
        });
    }
    #[unsafe(naked)]
    extern "C" fn return_to_user() {
        seq!(N in 3..=31 {
            naked_asm!(
                // 1. 将 stvec 改为用户态的版本
                "la t0, {trap_handler_raw_from_user}",
                "csrw stvec, t0",
                // 2. 恢复用户栈的 sp（到 sscratch 中）
                "ld a2, {REGS_OFFSET}+8(sp)",
                "csrw sscratch, a2",
                // 3. 恢复 sepc 和 sstatus
                "ld a1, {RET_PC_OFFSET}(sp)",
                "csrw sepc, a1",
                "ld a0, {STATUS_OFFSET}(sp)",
                "csrw sstatus, a0",
                // 4. 恢复先前保存的其它寄存器
                "ld x1, {REGS_OFFSET}(sp)",
                #(
                    concat!("ld x", N, ", (", N, "-1)*8+{REGS_OFFSET}(sp)"),
                )*
                // 5. 释放栈空间
                "addi sp, sp, {TRAP_CONTEXT_SIZE}",
                // 6. 切换回用户栈
                "csrrw sp, sscratch, sp",
                // 7. 返回用户态（U 特权级）
                "sret",
                TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
                REGS_OFFSET = const REGS_OFFSET,
                STATUS_OFFSET = const STATUS_OFFSET,
                RET_PC_OFFSET = const RET_PC_OFFSET,
                trap_handler_raw_from_user = sym Self::trap_handler_raw_from_user,
            );
        });
    }

    #[rustc_align(4)]
    #[unsafe(naked)]
    extern "C" fn trap_handler_raw_from_kernel() {
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

    fn get_bad_address() -> usize {
        let stval: usize;
        unsafe {
            asm!(
                "csrr {0}, stval",
                out(reg) stval,
                options(nostack, nomem, preserves_flags),
            );
        }
        stval
    }

    fn set_global_interrupts_enable(enable: bool) {
        if enable {
            unsafe { riscv::register::sstatus::set_sie() }
        } else {
            unsafe { riscv::register::sstatus::clear_sie() }
        }
    }

    fn enable_timer_interrupt() {
        unsafe { riscv::register::sie::set_stimer() };
    }
}

#[repr(align(16))] // 上下文须压栈，RISC-V 64 要求栈按 16 字节对齐
pub struct TrapContext {
    pub regs: Registers,
    pub status: usize,
    pub ret_pc: usize,
}

const TRAP_CONTEXT_SIZE: usize = size_of::<TrapContext>();
const REGS_OFFSET: usize = offset_of!(TrapContext, regs);
const STATUS_OFFSET: usize = offset_of!(TrapContext, status);
const RET_PC_OFFSET: usize = offset_of!(TrapContext, ret_pc);

macro_rules! regs {
    ($($name:ident),+ $(,)?) => {
        #[repr(C)]
        #[derive(Debug)]
        pub struct Registers {
            $(
                #[debug("{:#018x}", $name)]
                pub $name: usize,
            )+
        }
    };
}

regs!(
    ra, sp, gp, tp, t0, t1, t2, fp, s1, a0, a1, a2, a3, a4, a5, a6, a7, s2, s3, s4, s5, s6, s7, s8,
    s9, s10, s11, t3, t4, t5, t6
);

impl interfaces::TrapContext for TrapContext {
    fn syscall_id(&self) -> usize {
        self.regs.a7
    }

    fn syscall_args(&self) -> &[usize; 6] {
        let ptr: *const usize = &self.regs.a0;
        unsafe { ptr.cast::<[usize; 6]>().as_ref_unchecked() }
    }

    fn ret_reg(&self) -> usize {
        self.regs.a0
    }

    fn ret_reg_mut(&mut self) -> &mut usize {
        &mut self.regs.a0
    }

    fn is_trap_from_kernel(&self) -> bool {
        self.status.get_bit(8)
    }
}
