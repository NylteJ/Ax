pub unsafe trait Regs {
    type RegType: Copy = usize;

    fn get_fp() -> Self::RegType;

    fn get_tp() -> Self::RegType;
    fn set_tp(tp: Self::RegType);

    fn get_sp() -> Self::RegType;

    fn set_trap_handler(new_handler: extern "C" fn());
}
