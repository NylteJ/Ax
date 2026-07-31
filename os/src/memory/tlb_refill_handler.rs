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

/// 完成当前 CPU 上使用 TLB 所需的一切初始化（设置 TLBRENTRY、页表格式配置并清空 TLB）。
///
/// 这些 CSR 都是每核独立的，因此每个核（包括次要核）都必须调用一次。
pub fn init_cpu() {
    use bit_field::BitField;

    use crate::memory::PAGE_SHIFTS;

    init();

    unsafe {
        asm!(
            "csrwr {page_shift}, {STLBPS}",
            page_shift = inlateout(reg) PAGE_SHIFTS[0] => _,
            STLBPS = const 0x1e,
        );
    }

    unsafe {
        asm!(
            "csrwr {page_shift}, {TLBREHI}",
            page_shift = inlateout(reg) PAGE_SHIFTS[0] => _,
            TLBREHI = const 0x8e,
        );
    }

    // LoongArch 64 支持五级页表，格式也可以自定义，不过我们这里配成 Sv39 就够了
    {
        let mut pwcl = 0;
        pwcl.set_bits(0..=4, PAGE_SHIFTS[0]); // PTbase
        pwcl.set_bits(5..=9, PAGE_SHIFTS[1] - PAGE_SHIFTS[0]); // PTwidth
        pwcl.set_bits(10..=14, PAGE_SHIFTS[1]); // Dir1_base
        pwcl.set_bits(15..=19, PAGE_SHIFTS[2] - PAGE_SHIFTS[1]); // Dir1_width
        pwcl.set_bits(20..=24, PAGE_SHIFTS[2]); // Dir2_base
        pwcl.set_bits(25..=29, 9); // Dir2_width
        pwcl.set_bits(30..=31, 0); // PTEWidth

        let pwch = 0;

        unsafe {
            asm!(
                "csrwr {pwcl}, {PWCL}",
                "csrwr {pwch}, {PWCH}",
                pwcl = inlateout(reg) pwcl => _,
                pwch = inlateout(reg) pwch => _,
                PWCL = const 0x1c,
                PWCH = const 0x1d,
            );
        }
    }

    // 清空整个 TLB
    unsafe { asm!("invtlb 0x0, $zero, $zero") };
}
