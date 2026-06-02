use core::arch::asm;

use bit_field::BitField;

use crate::{
    arch::interfaces::{self, Trap},
    memory::address::{PhysAddr, VirtAddr},
};

pub struct Basic;

const CON_BASE_VADDR: VirtAddr = PhysAddr(0x1fe001e0).to_virtual_address();
const POWER_BASE_VADDR: VirtAddr = PhysAddr(0x100e001c).to_virtual_address();

impl interfaces::Basic for Basic {
    type PrintError = !;

    type InputError = !;

    #[inline]
    fn print_u8(chr: u8) -> Result<(), Self::PrintError> {
        unsafe { CON_BASE_VADDR.as_mut_ptr::<u8>().write_volatile(chr) };
        Ok(())
    }

    fn input_u8() -> Result<u8, Self::InputError> {
        let chr_raw = loop {
            let has_data: u8 = unsafe {
                VirtAddr(CON_BASE_VADDR.0 + 5)
                    .as_ptr::<u8>()
                    .read_volatile()
            };
            let has_data = has_data & 0x1 == 1;

            if has_data {
                break unsafe { CON_BASE_VADDR.as_ptr::<u8>().read_volatile() };
            }
        };

        if chr_raw == b'\r' {
            Ok(b'\n') // 将回车转换为换行
        } else {
            Ok(chr_raw)
        }
    }

    fn shutdown(normal: bool) -> ! {
        _ = normal; // 没有接口 :(

        // LoongArch 这里貌似有延迟，写成 unreachable 的话可能还会往下再 panic 一次
        // （甚至有时还会莫名半天关不掉，所以干脆写成死循环关机了）
        loop {
            unsafe { POWER_BASE_VADDR.as_mut_ptr::<u8>().write_volatile(0x34) };
        }
    }

    fn tlb_invalidate_addr(vaddr: VirtAddr) {
        unsafe {
            asm!(
                "invtlb 0x6, $zero, {vaddr}",
                vaddr = in(reg) vaddr.0,
                options(nostack, preserves_flags)
            );
        }
    }

    fn tlb_invalidate_all() {
        unsafe {
            asm!(
                "invtlb 0x0, $zero, $zero",
                options(nostack, preserves_flags)
            );
        }
    }

    unsafe fn execute_init(entry: usize, ustack: usize, kstack: usize) -> ! {
        let mut prmd: usize;
        unsafe { asm!("csrrd {0}, 0x0", out(reg) prmd) }; // CRMD
        prmd.set_bits(0..2, 3);
        prmd.set_bit(2, true);
        unsafe { asm!("csrwr {0}, 0x1", inlateout(reg) prmd => _) };

        unsafe {
            asm!(
                "csrwr {trap_handler_raw_from_user}, 0xc",
                "csrwr {entry}, 0x6",
                "csrwr {kstack}, 0x30",
                "move $sp, {ustack}",
                "ertn",
                trap_handler_raw_from_user = in(reg) super::trap::Trap::trap_handler_raw_from_user,
                entry = in(reg) entry,
                ustack = in(reg) ustack,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        }
    }
}
