use core::{
    arch::{asm, naked_asm},
    mem::offset_of,
};

use bit_field::BitField;
use derive_more::Debug;
use seq_macro::seq;

use crate::{arch::interfaces, trap::trap_handler::recover_task_pointer};

pub struct Trap;

const CSR_SAVE0: usize = 0x30;
const CSR_PRMD: usize = 0x1;
const CSR_ERA: usize = 0x6;
const CSR_EENTRY: usize = 0xc;

unsafe impl interfaces::Trap for Trap {
    type TrapContext = TrapContext;

    #[repr(align(4096))]
    #[naked]
    #[unsafe(no_mangle)]
    extern "C" fn trap_handler_raw_from_user() {
        unsafe {
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
                    "st.d $t0, $sp, {PRMD_OFFSET}",
                    "csrrd $t1, {CSR_ERA}",
                    "st.d $t1, $sp, {RET_PC_OFFSET}",
                    // 5. 保存用户栈的 $sp
                    "csrrd $t2, {CSR_SAVE0}",
                    "st.d $t2, $sp, {REGS_OFFSET}+2*8",
                    // 6. 将 EENTRY 改为内核态的版本
                    "la $t0, trap_handler_raw_from_kernel",
                    "csrwr $t0, {CSR_EENTRY}",
                    // 7. 恢复 tp
                    "bl {recover_task_pointer}",
                    // 8. 调用 trap_handler
                    "move $a0, $sp", // 传参
                    "bl trap_handler",
                    // 9. 返回用户态
                    "b return_to_user", // 尾调用，无条件跳转即可
                    CSR_SAVE0 = const CSR_SAVE0,
                    CSR_PRMD = const CSR_PRMD,
                    CSR_ERA = const CSR_ERA,
                    CSR_EENTRY = const CSR_EENTRY,
                    TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
                    REGS_OFFSET = const REGS_OFFSET,
                    PRMD_OFFSET = const PRMD_OFFSET,
                    RET_PC_OFFSET = const RET_PC_OFFSET,
                    recover_task_pointer = sym recover_task_pointer,
                );
            });
        }
    }
    #[naked]
    #[unsafe(no_mangle)]
    extern "C" fn return_to_user() {
        unsafe {
            seq!(N in 4..=31 {
                naked_asm!(
                    // 1. 将 EENTRY 改为用户态的版本
                    "la $t0, trap_handler_raw_from_user",
                    "csrwr $t0, {CSR_EENTRY}",
                    // 2. 恢复用户栈的 $sp（到 CSR_SAVE0 中）
                    "ld.d $t2, $sp, {REGS_OFFSET}+2*8",
                    "csrwr $t2, {CSR_SAVE0}",
                    // 3. 恢复 ERA 和 PRMD
                    "ld.d $t1, $sp, {RET_PC_OFFSET}",
                    "csrwr $t1, {CSR_ERA}",
                    "ld.d $t0, $sp, {PRMD_OFFSET}",
                    "csrwr $t0, {CSR_PRMD}",
                    // 4. 恢复先前保存的其它寄存器
                    "ld.d $r1, $sp, {REGS_OFFSET}",
                    "ld.d $r2, $sp, {REGS_OFFSET}+8",
                    #(
                        concat!("ld.d $r", N, ", $sp, (", N, "-1)*8+{REGS_OFFSET}"),
                    )*
                    // 5. 释放栈空间
                    "addi.d $sp, $sp, {TRAP_CONTEXT_SIZE}",
                    // 6. 切换回用户栈
                    "csrwr $sp, {CSR_SAVE0}", // 交换二者的值
                    // 7. 返回用户态
                    "ertn",
                    CSR_SAVE0 = const CSR_SAVE0,
                    CSR_PRMD = const CSR_PRMD,
                    CSR_ERA = const CSR_ERA,
                    CSR_EENTRY = const CSR_EENTRY,
                    TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
                    REGS_OFFSET = const REGS_OFFSET,
                    PRMD_OFFSET = const PRMD_OFFSET,
                    RET_PC_OFFSET = const RET_PC_OFFSET,
                );
            });
        }
    }

    #[repr(align(4096))]
    #[naked]
    #[unsafe(no_mangle)]
    extern "C" fn trap_handler_raw_from_kernel() {
        unsafe {
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
                    "st.d $t0, $sp, {PRMD_OFFSET}",
                    "csrrd $t1, {CSR_ERA}",
                    "st.d $t1, $sp, {RET_PC_OFFSET}",
                    // 4. 调用 trap_handler
                    "move $a0, $sp", // 传参
                    "bl trap_handler",
                    // 5. 恢复 ERA 和 PRMD
                    "ld.d $t1, $sp, {RET_PC_OFFSET}",
                    "csrwr $t1, {CSR_ERA}",
                    "ld.d $t0, $sp, {PRMD_OFFSET}",
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
                    CSR_PRMD = const CSR_PRMD,
                    CSR_ERA = const CSR_ERA,
                    TRAP_CONTEXT_SIZE = const TRAP_CONTEXT_SIZE,
                    REGS_OFFSET = const REGS_OFFSET,
                    PRMD_OFFSET = const PRMD_OFFSET,
                    RET_PC_OFFSET = const RET_PC_OFFSET,
                );
            });
        }
    }

    fn get_bad_address() -> usize {
        let badv: usize;
        unsafe { asm!("csrrd {0}, 0x7", out(reg) badv, options(nostack, nomem, preserves_flags)) };
        badv
    }

    fn set_global_interrupts_enable(enable: bool) {
        let mut crmd: usize;
        unsafe { asm!("csrrd {0}, 0x0", out(reg) crmd) };

        crmd.set_bit(2, enable);

        unsafe { asm!("csrwr {0}, 0x0", inlateout(reg) crmd => _) };
    }

    fn enable_timer_interrupt() {
        const ECFG: usize = 1 << (crate::trap::trap::Interrupt::TimerInterrupt as usize);
        unsafe { asm!("csrwr {0}, 0x4", inlateout(reg) ECFG => _) }
    }
}

#[repr(align(16))] // 上下文须压栈，LoongArch 64 要求栈按 16 字节对齐
pub struct TrapContext {
    pub regs: Registers,
    pub prmd: usize,
    pub ret_pc: usize,
}

const TRAP_CONTEXT_SIZE: usize = size_of::<TrapContext>();
const REGS_OFFSET: usize = offset_of!(TrapContext, regs);
const PRMD_OFFSET: usize = offset_of!(TrapContext, prmd);
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
    ra, tp, sp, a0, a1, a2, a3, a4, a5, a6, a7, t0, t1, t2, t3, t4, t5, t6, t7, t8, r21, fp, s0,
    s1, s2, s3, s4, s5, s6, s7, s8
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
        self.prmd.get_bits(0..2) == 0
    }
}
