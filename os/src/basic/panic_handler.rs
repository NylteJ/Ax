use core::fmt::Write;
use core::panic::PanicInfo;

use crate::arch::interfaces::Basic;
use crate::basic::console::{CONSOLE_WRITER, ConsoleWriter};
use crate::basic::debugger::{backtrace, debugbreak};
use crate::{arch, println};

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if CONSOLE_WRITER.try_lock().is_some() {
        println!("Kernel panic: {info}");
        backtrace();
    } else {
        let _ = writeln!(ConsoleWriter, "Kernel {info}");
    }

    debugbreak();
    arch::NativeBasic::shutdown(false);
}
