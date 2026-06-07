use core::arch::asm;

use riscv::register::sstatus;

use crate::{
    arch::interfaces::{self, Trap},
    memory::address::VirtAddr,
};

pub struct Basic;

impl interfaces::Basic for Basic {
    type PrintError = usize;

    type InputError = !;

    #[inline]
    fn print_u8(chr: u8) -> Result<(), Self::PrintError> {
        let code;

        unsafe {
            asm!(
                "ecall",
                in("a7") 0x01, // EID = sbi_console_putchar
                // FID = ignored
                in("a0") chr,
                lateout("a0") code,
                options(nostack, nomem, preserves_flags),
            );
        }

        if code == 0 { Ok(()) } else { Err(code) }
    }

    fn input_u8() -> Result<u8, Self::InputError> {
        let chr_raw = loop {
            let code: isize;

            unsafe {
                asm!("ecall",
                    in("a7") 0x02, // EID = sbi_console_getchar
                    // FID = ignored
                    lateout("a0") code,
                    options(nostack, nomem, preserves_flags),
                );
            }

            if code != -1 {
                break code as u8;
            }
        };

        if chr_raw == b'\r' {
            Ok(b'\n') // 将回车转换为换行
        } else {
            Ok(chr_raw)
        }
    }

    fn shutdown(normal: bool) -> ! {
        let value = if normal {
            0x00000000 // No reason
        } else {
            0x00000001 // System failure
        };

        unsafe {
            asm!("ecall",
                in("a7") 0x53525354, // EID = SRST
                in("a6") 0x00, // FID = sbi_system_reset
                in("a0") 0x00, // reset_type = Shutdown
                in("a1") value,
            );
        }

        unreachable!()
    }

    fn tlb_invalidate_addr(vaddr: VirtAddr) {
        unsafe {
            asm!(
                "sfence.vma {vaddr}, zero",
                vaddr = in(reg) vaddr.0,
                options(nostack, preserves_flags)
            );
        }
    }

    fn tlb_invalidate_all() {
        unsafe {
            asm!("sfence.vma zero, zero", options(nostack, preserves_flags));
        }
    }

    unsafe fn execute_init(entry: usize, ustack: usize, kstack: usize) -> ! {
        unsafe { sstatus::set_spp(sstatus::SPP::User) };

        unsafe {
            asm!(
                "csrw stvec, {trap_handler_raw_from_user}",
                "csrw sepc, {entry}",
                "csrw sscratch, {ustack}",
                "csrrw sp, sscratch, {kstack}",
                "sret",
                trap_handler_raw_from_user = in(reg) super::trap::Trap::trap_handler_raw_from_user,
                entry = in(reg) entry,
                ustack = in(reg) ustack,
                kstack = in(reg) kstack,
                options(noreturn),
            );
        }
    }

    #[inline]
    fn halt() {
        unsafe { asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}
