use core::arch::asm;

use crate::arch::interfaces;

pub struct Regs;

unsafe impl interfaces::Regs for Regs {
    fn get_fp() -> Self::RegType {
        let fp;
        unsafe { asm!("move {0}, $fp", out(reg) fp, options(nostack, nomem, preserves_flags)) };
        fp
    }

    fn get_tp() -> Self::RegType {
        let tp;
        unsafe { asm!("move {0}, $tp", out(reg) tp, options(nostack, nomem, preserves_flags)) };
        tp
    }
    fn set_tp(tp: Self::RegType) {
        unsafe { asm!("move $tp, {0}", in(reg) tp, options(nostack, nomem, preserves_flags)) };
    }

    fn get_sp() -> Self::RegType {
        let sp;
        unsafe { asm!("move {0}, $sp", out(reg) sp, options(nostack, nomem, preserves_flags)) };
        sp
    }

    fn set_trap_handler(new_handler: extern "C" fn()) {
        unsafe {
            asm!("csrwr {0}, 0xc", inlateout(reg) new_handler => _, options(nostack, nomem, preserves_flags))
        };
    }
}
