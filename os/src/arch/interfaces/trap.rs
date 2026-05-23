pub unsafe trait Trap {
    type TrapContext: TrapContext;

    extern "C" fn trap_handler_raw_from_user();
    extern "C" fn return_to_user();

    extern "C" fn trap_handler_raw_from_kernel();

    fn get_bad_address() -> usize;

    fn set_global_interrupts_enable(enable: bool);
    fn enable_timer_interrupt();
}

pub trait TrapContext {
    fn syscall_id(&self) -> usize;
    fn syscall_args(&self) -> &[usize; 6];

    fn ret_reg(&self) -> usize;
    fn ret_reg_mut(&mut self) -> &mut usize;

    fn is_trap_from_kernel(&self) -> bool;
}
