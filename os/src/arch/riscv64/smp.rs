use core::arch::asm;

use crate::{
    arch::interfaces::Smp,
    basic::per_cpu::MAX_CPUS,
    memory::address::LINEAR_MAPPING_OFFSET,
    println,
};

/// 调用 SBI 的 `sbi_hart_start`，将指定 hart 从 OpenSBI 的等待循环中唤醒，并跳转到 `start_addr` 执行。
///
/// `start_addr` 必须是物理地址，因为此时新 hart 的 satp 还是 Bare 模式。
fn sbi_hart_start(hartid: usize, start_addr: usize, opaque: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "ecall",
            in("a7") 0x48534d, // EID = HSM
            in("a6") 0x0,      // FID = sbi_hart_start
            in("a0") hartid,
            in("a1") start_addr,
            in("a2") opaque,
            lateout("a0") ret,
            options(nostack, nomem, preserves_flags),
        );
    }
    ret
}

pub struct SmpImpl;

unsafe impl Smp for SmpImpl {
    fn start_secondary_cpus() {
        let entry: usize;
        unsafe {
            asm!("la {0}, {secondary_start}", out(reg) entry, secondary_start = sym crate::arch::riscv64::secondary_start);
        }
        let entry_phys = entry - LINEAR_MAPPING_OFFSET;

        for hartid in 1..MAX_CPUS {
            let ret = sbi_hart_start(hartid, entry_phys, 0);
            if ret == 0 {
                log::info!("已请求启动 hart {hartid}");
            } else {
                log::warn!("启动 hart {hartid} 失败：SBI 返回 {ret:#x}（可能不存在该 hart）");
            }
        }
    }
}
