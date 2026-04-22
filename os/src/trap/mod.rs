pub mod trap_handler;

mod trap;

pub fn set_global_interrupts_enable(enable: bool) {
    cfg_select! {
        target_arch = "riscv64" => if enable {
            unsafe { riscv::register::sstatus::set_sie() }
        } else {
            unsafe { riscv::register::sstatus::clear_sie() }
        },
        target_arch = "loongarch64" => {
            use core::arch::asm;
            use bit_field::BitField;

            let mut crmd: usize;
            unsafe {asm!("csrrd {0}, 0x0", out(reg) crmd)};

            crmd.set_bit(2, enable);

            unsafe { asm!("csrwr {0}, 0x0", in(reg) crmd) };
        },
    }
}

pub fn enable_timer_interrupt() {
    cfg_select! {
        target_arch = "riscv64" => unsafe { riscv::register::sie::set_stimer(); },
        target_arch = "loongarch64" => {
            use core::arch::asm;

            const ECFG: usize = 1 << (trap::Interrupt::TimerInterrupt as usize);
            unsafe { asm!("csrwr {0}, 0x4", in(reg) ECFG) }
        },
    }
}
