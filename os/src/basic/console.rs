use core::fmt::Write;

use crate::basic::sbi::{SbiError, putchar_ascii};
use crate::basic::sync::Mutex;

pub struct ConsoleWriter;

pub static CONSOLE_WRITER: Mutex<ConsoleWriter> = Mutex::new(ConsoleWriter);

impl Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            putchar_ascii(byte).map_err(|_| core::fmt::Error)?;
        }
        Ok(())
    }
}

#[macro_export]
#[rustc_diagnostic_item = "print_macro"]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {{
        use core::fmt::Write;
        write!($crate::basic::console::CONSOLE_WRITER.lock(), $fmt $(, $($arg)+)?).unwrap();
    }};
}

#[macro_export]
#[rustc_diagnostic_item = "println_macro"]
macro_rules! println {
    () => {
        $crate::basic::sbi::putchar_ascii('\n' as u8).unwrap();
    };
    ($fmt: literal $(, $($arg: tt)+)?) => {{
        use core::fmt::Write;
        writeln!($crate::basic::console::CONSOLE_WRITER.lock(), $fmt $(, $($arg)+)?).unwrap();
    }};
}
