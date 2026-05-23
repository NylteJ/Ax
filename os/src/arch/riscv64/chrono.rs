use core::arch::asm;

use crate::arch::interfaces;

pub struct Chrono;

impl interfaces::Chrono for Chrono {
    type CpuTickType = usize;

    fn get_cpu_tick() -> Self::CpuTickType {
        riscv::register::time::read()
    }

    fn get_clock_freq() -> Self::CpuTickType {
        CLOCK_FREQ
    }

    fn start_timer_tick_tock(interrupt_per_sec: usize) {
        let after = CLOCK_FREQ / interrupt_per_sec;
        interrupt_when(Self::get_cpu_tick() + after);
    }

    fn reset_timer_interrupt(interrupt_per_sec: usize) {
        Self::start_timer_tick_tock(interrupt_per_sec);
    }
}

const CLOCK_FREQ: usize = 12500000;

fn interrupt_when(tick: usize) {
    unsafe {
        asm!("ecall",
            in("a7") 0x54494d45, // EID = TIME
            in("a6") 0x0, // FID = sbi_set_timer
            in("a0") tick,
            options(nostack, nomem, preserves_flags),
        );
    }
}
