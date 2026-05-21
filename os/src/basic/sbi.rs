#[cfg(target_arch = "riscv64")]
use core::arch::asm;

#[cfg(target_arch = "loongarch64")]
use crate::memory::address::PhysAddr;
#[cfg(target_arch = "loongarch64")]
use core::ptr::{read_volatile, write_volatile};

pub struct SbiError {
    pub code: isize,
}

impl SbiError {
    pub fn message(&self) -> &'static str {
        cfg_select! {
            target_arch = "riscv64" => {
                const STANDARD_RISCV_ERROR_CODE_TABLE: [&'static str; 15] = [
                    "成功完成 (Completed successfully)", // 尽管一般来说你不应该看到这个
                    "失败 (Failed)",
                    "不支持 (Not supported)",
                    "无效参数 (Invalid parameter(s))",
                    "被拒绝或不允许 (Denied or not allowed)",
                    "无效地址 (Invalid address(s))",
                    "已经可用 (Already available)",
                    "已经启动 (Already started)",
                    "已经停止 (Already stopped)",
                    "共享内存不可用 (Shared memory not available)",
                    "无效状态 (Invalid state)",
                    "错误（或无效）范围 (Bad (or invalid) range)",
                    "因超时而失败 (Failed due to timeout)",
                    "输入/输出错误 (Input/Output error)",
                    "由于锁定状态而被拒绝或不允许 (Denied or not allowed due to lock status)",
                ];
                STANDARD_RISCV_ERROR_CODE_TABLE.get(-self.code as usize).unwrap_or(&"未知错误 (Unknown error)")
            },
            target_arch = "loongarch64" => todo!(),
        }
    }
}

impl core::fmt::Debug for SbiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SBI 错误，{}（代码 {}）", self.message(), self.code)
    }
}

pub fn putchar_u8(chr: u8) -> Result<(), SbiError> {
    cfg_select! {
        target_arch = "riscv64" => {
            let code;

            unsafe {
                asm!("ecall",
                    in("a7") 0x01, // EID = sbi_console_putchar
                    // FID = ignored
                    inlateout("a0") chr as usize => code,
                );
            }

            if code == 0 {
                Ok(())
            } else {
                Err(SbiError { code })
            }
        },
        target_arch = "loongarch64" => {
            unsafe { write_volatile(PhysAddr(0x1fe001e0).to_virtual_address().as_mut_ptr(), chr) };
            Ok(())
        },
    }
}

/// 从控制台读取一个字节的数据。
///
/// 此函数需要等待用户输入并且很可能会阻塞。阻塞是同步的（因为我们只有一个控制台），基本上就是空转。
pub fn getchar_u8() -> Result<u8, SbiError> {
    // 直接在内核态空转，因为输入其实不是我们内核的核心功能
    let chr_raw = loop {
        cfg_select! {
            target_arch = "riscv64" => {
                let code: i64;

                unsafe {
                    asm!("ecall",
                        in("a7") 0x02, // EID = sbi_console_getchar
                        // FID = ignored
                        out("a0") code,
                    );
                }

                if code != -1 {
                    break code as u8;
                }
            },
            target_arch = "loongarch64" => {
                let has_data: u8 = unsafe { read_volatile(PhysAddr(0x1fe001e5).to_virtual_address().as_ptr()) };
                let has_data = has_data & 0x1 == 1;

                if has_data {
                    break unsafe { read_volatile(PhysAddr(0x1fe001e0).to_virtual_address().as_ptr()) };
                }
            },
        }
    };

    if chr_raw == b'\r' {
        Ok(b'\n') // 将回车转换为换行
    } else {
        Ok(chr_raw)
    }
}

pub enum ShutdownStatus {
    Normal = 0x00000000, // No reason
    Error = 0x00000001,  // System failure
}

pub fn shutdown(status: ShutdownStatus) -> ! {
    cfg_select! {
        target_arch = "riscv64" => {
            unsafe {
                asm!("ecall",
                    in("a7") 0x53525354, // EID = SRST
                    in("a6") 0x00, // FID = sbi_system_reset
                    in("a0") 0x00, // reset_type = Shutdown
                    in("a1") status as usize,
                );
            }

            unreachable!()
        },
        target_arch = "loongarch64" => {
            _ = status; // 没有接口 :(

            unsafe { write_volatile(PhysAddr(0x100e001c).to_virtual_address().0 as *mut u8, 0x34) };

            loop {} // LoongArch 这里貌似有延迟，写成 unreachable 的话可能还会往下再 panic 一次
        },
    }
}
