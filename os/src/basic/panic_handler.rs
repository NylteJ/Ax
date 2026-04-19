use core::panic::PanicInfo;

use crate::basic::debugger::backtrace;
// use crate::basic::debugger::debugbreak;
use crate::basic::sbi::ShutdownStatus::Error;
use crate::basic::sbi::shutdown;
use crate::println;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("Kernel {}", info);
    backtrace();
    // debugbreak();
    shutdown(Error);
}
