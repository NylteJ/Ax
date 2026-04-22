use core::{arch::asm, ptr};

use crate::println;

pub fn debugbreak() {
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("ebreak") },
        target_arch = "loongarch64" => unsafe { asm!("break 0") },
    };
}

/// 回溯打印当前调用栈（当然，是没有符号的版本）。
///
/// 目前实现依赖于帧指针。
#[cfg(feature = "framepointer")]
pub fn backtrace() {
    let mut frame_pointer: *const usize;
    cfg_select! {
        target_arch = "riscv64" =>     unsafe { asm!("mv   {0}, s0",  out(reg) frame_pointer) },
        target_arch = "loongarch64" => unsafe { asm!("move {0}, $fp", out(reg) frame_pointer) },
    }

    if frame_pointer == ptr::null() {
        println!("ERROR: 帧指针为空");
        return;
    }

    for depth in 0..100 {
        let pc = unsafe { *frame_pointer.sub(1) };
        let fp = unsafe { *frame_pointer.sub(2) };

        if cfg_select! {
            target_arch = "riscv64" => fp < 0x80200000, // RISC-V 这部分貌似会一路回溯到 SBI 里头，这部分我们不感兴趣
            target_arch = "loongarch64" => fp == 0, // 到达调用链末尾
        } {
            break;
        }

        println!("#{depth:2} {pc:#18x}, fp = {fp:#18x}");

        frame_pointer = fp as *const usize;
    }
}
