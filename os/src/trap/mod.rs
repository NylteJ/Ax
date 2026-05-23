pub mod page_fault_type;
pub mod trap_handler;

pub mod trap;

pub fn set_global_interrupts_enable(enable: bool) {
    use crate::arch::{self, interfaces::Trap};

    arch::NativeTrap::set_global_interrupts_enable(enable);
}

pub fn enable_timer_interrupt() {
    use crate::arch::{self, interfaces::Trap};

    arch::NativeTrap::enable_timer_interrupt();
}
