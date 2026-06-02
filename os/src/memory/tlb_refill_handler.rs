#![cfg(target_arch = "loongarch64")]

use core::arch::{asm, naked_asm};

use crate::memory::address::VirtAddr;

#[repr(align(4096))]
#[naked]
extern "C" fn tlb_refill_handler() {
    unsafe {
        naked_asm!(
            // 和 trap_handler 等地方不同，TLB 填充的逻辑非常简单（且可能被很频繁地触发）因此不需要保存和恢复完整上下文
            // 但相同的是硬件依旧只负责跳转，还是需要保留原来的各个寄存器的
            "csrwr $t0, {TLBRSAVE}", // 保存 t0 原值
            "csrrd $t0, {PGD}", // 加载对应页表
            "lddir $t0, $t0, 2", // Dir1
            "bstrins.d $t0, $zero, 11, 0", // 清零低 12 位
            "lddir $t0, $t0, 1", // PT
            "bstrins.d $t0, $zero, 11, 0", // 清零低 12 位
            "ldpte $t0, 0", // TLBRELO0
            "ldpte $t0, 1", // TLBRELO1
            "tlbfill", // 填充 TLB
            "csrrd $t0, {TLBRSAVE}", // 恢复 t0 原值
            "ertn", // 返回！
            TLBRSAVE = const 0x8b,
            PGD = const 0x1b,
        );
    }
}

pub fn init() {
    let vaddr = VirtAddr(tlb_refill_handler as usize);
    let paddr = vaddr.to_physical_address().0;

    unsafe { asm!("csrwr {0}, 0x88", inlateout(reg) paddr => _) }
}
