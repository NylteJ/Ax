use core::fmt::Write;

use crate::basic::sbi::SbiError;
use crate::basic::sync::RwLock;

#[derive(Debug)]
pub enum ConsoleIoError {
    SbiError(SbiError),
}

impl From<SbiError> for ConsoleIoError {
    fn from(sbi_error: SbiError) -> Self {
        ConsoleIoError::SbiError(sbi_error)
    }
}

pub fn print_to_console(text: &str) -> Result<(), ConsoleIoError> {
    for byte in text.bytes() {
        crate::basic::sbi::putchar_ascii(byte)?;
    }
    Ok(())
}

pub struct ConsoleWriter;

pub static CONSOLE_WRITER: RwLock<ConsoleWriter> = RwLock::new(ConsoleWriter);

impl Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        print_to_console(s).map_err(|_| core::fmt::Error)
    }
}

#[macro_export]
#[rustc_diagnostic_item = "print_macro"]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {{
        use core::fmt::Write;
        write!($crate::basic::console::CONSOLE_WRITER.write(), $fmt $(, $($arg)+)?).unwrap();
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
        writeln!($crate::basic::console::CONSOLE_WRITER.write(), $fmt $(, $($arg)+)?).unwrap();
    }};
}
