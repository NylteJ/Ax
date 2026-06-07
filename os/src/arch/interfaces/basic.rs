use crate::memory::address::VirtAddr;

pub trait Basic {
    type PrintError;
    type InputError;

    /// 将一个字节的数据输出到控制台。
    fn print_u8(chr: u8) -> Result<(), Self::PrintError>;
    /// 从控制台读取一个字节的数据。
    ///
    /// 此函数需要等待用户输入并且很可能会阻塞。阻塞是同步的（因为我们只有一个控制台），基本上就是空转。
    fn input_u8() -> Result<u8, Self::InputError>;

    fn shutdown(normal: bool) -> !;

    fn tlb_invalidate_addr(vaddr: VirtAddr);
    fn tlb_invalidate_all();

    unsafe fn execute_init(entry: usize, ustack: usize, kstack: usize) -> !;

    /// 提示 CPU 进入空闲状态，直到下一次中断到来（无论中断是否被屏蔽）。
    ///
    /// 这只是提示，不一定能被强制执行，且在内核层面无法感知。
    #[inline(always)]
    fn halt() {}
}
