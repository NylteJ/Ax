use core::arch::asm;

use lazy_static::lazy_static;

use crate::println;

pub fn cpu_tick_to_us(tick: usize) -> usize {
    tick / (*CLOCK_FREQ / 1000000) // 想你了 std::chrono 😭
}

pub fn us_to_cpu_tick(us: usize) -> usize {
    us * (*CLOCK_FREQ / 1000000)
}

pub fn get_cpu_tick() -> usize {
    cfg_select! {
        target_arch = "riscv64" => riscv::register::time::read(),
        target_arch = "loongarch64" => {
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
    }
}

const TIMER_INTERRUPT_PER_SECOND: usize = 1000;

/// 启动时钟中断。此函数只应被调用一次，后续应当使用 `reset_timer_interrupt`。
pub fn start_timer_tick_tock() {
    let after = *CLOCK_FREQ / TIMER_INTERRUPT_PER_SECOND;

    cfg_select! {
        target_arch = "riscv64" => riscv_timer_interrupt_when(get_cpu_tick() + after),
        target_arch = "loongarch64" => {
            use bit_field::BitField;

            let mut tcfg = 0usize;

            tcfg.set_bit(0, true); // 开启时钟中断
            tcfg.set_bit(1, true); // 周期性
            tcfg.set_bits(2.., after >> 2); // 定时器初始值

            unsafe { asm!("csrwr {0}, 0x41", in(reg) tcfg) }
        },
    }
}

/// 调整时钟中断使其仍会继续稳定触发
pub fn reset_timer_interrupt() {
    cfg_select! {
        target_arch = "riscv64" => {
            riscv_timer_interrupt_when(get_cpu_tick() + *CLOCK_FREQ / TIMER_INTERRUPT_PER_SECOND);
        },
        target_arch = "loongarch64" => {
            // LoongArch 上要手动清除时钟中断标志，但不需要设置下一个触发时间，因为我们使用的是周期模式
            unsafe { asm!("csrwr {0} , 0x44", in(reg) 1) }
        },
    }
}

// Performance Note: 或许可以移除 lazy_static，改为普通 static，手动 unsafe write_volatile 初始化
lazy_static! {
    static ref CLOCK_FREQ: usize = {
        let freq = {
            cfg_select! {
                target_arch = "riscv64" => 12500000,
                target_arch = "loongarch64" => {
                    use bit_field::BitField;

                    let base_freq: u32;
                    unsafe {
                        asm!("cpucfg {},{}", out(reg) base_freq, in(reg) 0x4);
                    }

                    let mul_div: u32;
                    unsafe {
                        asm!("cpucfg {},{}", out(reg) mul_div, in(reg) 0x5);
                    }
                    let mul = mul_div.get_bits(0..16) as u16;
                    let div = mul_div.get_bits(16..32) as u16;

                    base_freq as usize * mul as usize / div as usize
                },
            }
        };

        println!("时钟频率：{freq} Hz");
        freq
    };
}

#[cfg(target_arch = "riscv64")]
fn riscv_timer_interrupt_when(tick: usize) {
    unsafe {
        asm!("ecall",
            in("a7") 0x54494d45, // EID = TIME
            in("a6") 0x0, // FID = sbi_set_timer
            in("a0") tick,
        );
    }
}
