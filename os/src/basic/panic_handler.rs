use core::fmt::Write;
use core::panic::PanicInfo;

use crate::basic::console::CONSOLE_WRITER;
// use crate::basic::debugger::debugbreak;
use crate::basic::sbi::ShutdownStatus::Error;
use crate::basic::sbi::shutdown;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let _ = writeln!(
        unsafe { CONSOLE_WRITER.as_ptr().as_mut_unchecked() },
        "Kernel {info}"
    );
    // backtrace();
    // debugbreak();
    shutdown(Error);
}
