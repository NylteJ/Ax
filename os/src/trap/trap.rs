use num_enum::TryFromPrimitive;

#[derive(Debug)]
pub enum BasicTrap<I, E> {
    Interrupt(I),
    Exception(E),
}

pub type Trap = BasicTrap<Interrupt, Exception>;

impl Trap {
    /// 获取 Trap 实例。
    ///
    /// 很神奇但完全不需要输入参数，它能从虚空中提取信息！
    pub fn new() -> Self {
        use core::convert::TryFrom;

        cfg_select! {
            target_arch = "riscv64" => {
                match riscv::register::scause::read().cause() {
                    riscv::interrupt::Trap::Interrupt(code) => {
                        Self::Interrupt(Interrupt::try_from(code).unwrap())
                    }
                    riscv::interrupt::Trap::Exception(code) => {
                        Self::Exception(Exception::try_from(code).unwrap())
                    }
                }
            },
            target_arch = "loongarch64" => {
                use bit_field::BitField;
                use core::arch::asm;

                let estat: usize;
                unsafe { asm!("csrrd {0}, 0x5", out(reg) estat) };

                let ecode = estat.get_bits(16..=21);
                if ecode != 0 {
                    Self::Exception(Exception::try_from(ecode).unwrap())
                } else {
                    // “仅当 CSR.ECFG.VS=0 时，表示是中断”，但手册也没说不是 0 时的行为，当作未定义行为忽略了
                    let is = estat.get_bits(0..=12);

                    static_assertions::const_assert_eq!(size_of::<usize>(), 8);
                    Self::Interrupt(Interrupt::try_from((63 - is.leading_zeros()) as usize).unwrap())
                }
            },
        }
    }
}

#[cfg(target_arch = "riscv64")]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(usize)]
pub enum Interrupt {
    SupervisorSoft = 1,
    MachineSoft = 3,
    MachineTimer = 7,
    SupervisorExternal = 9,
    MachineExternal = 11,

    TimerInterrupt = 5,
}

#[cfg(target_arch = "riscv64")]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(usize)]
pub enum Exception {
    InstructionMisaligned = 0,
    InstructionFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadMisaligned = 4,
    LoadFault = 5,
    StoreMisaligned = 6,
    StoreFault = 7,
    SupervisorEnvCall = 9,
    MachineEnvCall = 11,

    Syscall = 8, // 即 UserEnvCall
    LoadPageFault = 13,
    StorePageFault = 15,
    InstructionPageFault = 12,
}

#[cfg(target_arch = "loongarch64")]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(usize)]
pub enum Interrupt {
    SoftwareInterrupt0 = 0,
    SoftwareInterrupt1 = 1,
    HardwareInterrupt0 = 2,
    HardwareInterrupt1 = 3,
    HardwareInterrupt2 = 4,
    HardwareInterrupt3 = 5,
    HardwareInterrupt4 = 6,
    HardwareInterrupt5 = 7,
    HardwareInterrupt6 = 8,
    HardwareInterrupt7 = 9,
    PerformanceMonitorCounterOverflow = 10,
    InterProcessorInterrupt = 12,

    TimerInterrupt = 11,
}

#[cfg(target_arch = "loongarch64")]
#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(usize)]
pub enum Exception {
    PageModifyFault = 0x4,
    PageNonReadableFault = 0x5,
    PageNonExecutableFault = 0x6,
    PagePrivilegeIllegal = 0x7,
    AddressError = 0x8,
    AddressNotAligned = 0x9,
    BoundsCheckFault = 0xA,
    Breakpoint = 0xC,
    InstructionNonDefined = 0xD,
    InstructionPrivilegeIllegal = 0xE,
    FloatingPointUnavailable = 0xF,

    LoadPageFault = 0x1,        // 即 PageInvalidLoad
    StorePageFault = 0x2,       // 即 PageInvalidStore
    InstructionPageFault = 0x3, // 即 PageInvalidFetch
    Syscall = 0xB,
}

impl Exception {
    pub fn is_page_fault(&self) -> bool {
        let v1 = matches!(
            self,
            Self::LoadPageFault | Self::StorePageFault | Self::InstructionPageFault
        );
        cfg_select! {
            target_arch = "riscv64" => v1,
            target_arch = "loongarch64" => v1 || matches!(self, Self::PageModifyFault | Self::PageNonReadableFault | Self::PageNonExecutableFault),
        }
    }
}
