use core::arch::asm;

use bit_field::BitField;

use crate::arch::interfaces;

pub struct Chrono;

impl interfaces::Chrono for Chrono {
    type CpuTickType = usize;

    fn get_cpu_tick() -> Self::CpuTickType {
        let ret;

        unsafe {
            asm!(
                "rdtime.d {tick}, {id}",
                tick = out(reg) ret,
                id = out(reg) _,
            );
        }

        ret
    }

    fn get_clock_freq() -> Self::CpuTickType {
        let base_freq: u32;
        unsafe { asm!("cpucfg {},{}", out(reg) base_freq, in(reg) 0x4) };

        let mul_div: u32;
        unsafe { asm!("cpucfg {},{}", out(reg) mul_div, in(reg) 0x5) };
        let mul = mul_div.get_bits(0..16) as u16;
        let div = mul_div.get_bits(16..32) as u16;

        base_freq as usize * mul as usize / div as usize
    }

    fn start_timer_tick_tock(interrupt_per_sec: usize) {
        let after = Self::get_clock_freq() / interrupt_per_sec;

        let mut tcfg = 0usize;

        tcfg.set_bit(0, true); // 开启时钟中断
        tcfg.set_bit(1, true); // 周期性
        tcfg.set_bits(2.., after >> 2); // 定时器初始值

        unsafe { asm!("csrwr {0}, 0x41", in(reg) tcfg) };
    }

    fn reset_timer_interrupt(_interrupt_per_sec: usize) {
        // 这里要手动清除时钟中断标志，但不需要设置下一个触发时间，因为我们使用的是周期模式
        unsafe { asm!("csrwr {0} , 0x44", in(reg) 1, options(nostack, nomem, preserves_flags)) };
    }
}
