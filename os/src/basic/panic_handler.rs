use core::fmt::Write;
use core::panic::PanicInfo;

use crate::basic::console::CONSOLE_WRITER;
use crate::basic::debugger::backtrace;
// use crate::basic::debugger::debugbreak;
use crate::basic::sbi::ShutdownStatus::Error;
use crate::basic::sbi::shutdown;
use crate::println;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if CONSOLE_WRITER.try_lock().is_some() {
        println!("Kernel panic: {info}");
        backtrace();
    } else {
        let _ = writeln!(
            unsafe { CONSOLE_WRITER.as_ptr().as_mut_unchecked() },
            "Kernel {info}"
        );
    }
    shutdown(Error);
}
