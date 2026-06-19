use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::interfaces::Basic;
use crate::basic::console::CONSOLE_WRITER;
use crate::basic::debugger::{backtrace, debugbreak};
use crate::{arch, println};

static PANICKING: AtomicBool = AtomicBool::new(false);

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if !PANICKING.swap(true, Ordering::Relaxed) {
        // 防止重复 panic 导致的死循环等问题

        // SAFETY: 其实都不安全，但我们已经要 panic 了，这样至少能提供更好的调试信息
        unsafe { CONSOLE_WRITER.assume_unlocked() };
        #[cfg(debug_assertions)] // 只在打印调试信息时需要堆分配
        unsafe {
            crate::basic::global_allocator::force_unlock_heap()
        };

        println!("Kernel panic: {info}");
        backtrace();

        debugbreak();
    }

    arch::NativeBasic::shutdown(false);
}
