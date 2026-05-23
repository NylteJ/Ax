use core::arch::asm;

use crate::arch::interfaces;

pub struct Regs;

unsafe impl interfaces::Regs for Regs {
    fn get_fp() -> Self::RegType {
        let fp;
        unsafe { asm!("mv {0}, s0", out(reg) fp, options(nostack, nomem, preserves_flags)) };
        fp
    }

    fn get_tp() -> Self::RegType {
        let tp;
        unsafe { asm!("mv {0}, tp", out(reg) tp, options(nostack, nomem, preserves_flags)) };
        tp
    }
    fn set_tp(tp: Self::RegType) {
        unsafe { asm!("mv tp, {0}", in(reg) tp, options(nostack, nomem, preserves_flags)) };
    }

    fn get_sp() -> Self::RegType {
        let sp;
        unsafe { asm!("mv {0}, sp", out(reg) sp, options(nostack, nomem, preserves_flags)) };
        sp
    }

    fn set_trap_handler(new_handler: extern "C" fn()) {
        unsafe {
            asm!("csrw stvec, {0}", in(reg) new_handler, options(nostack, nomem, preserves_flags))
        };
    }
}
