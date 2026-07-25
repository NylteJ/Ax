use crate::println;

pub struct Logger;

impl log::Log for Logger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        false
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = match record.level() {
            log::Level::Error => "\x1b[38;2;200;36;36m[ERROR]\x1b[0m",
            log::Level::Warn => "\x1b[38;2;220;200;48m[ WARN]\x1b[0m",
            log::Level::Info => "\x1b[38;2;45;50;220m[ INFO]\x1b[0m",
            log::Level::Debug => "\x1b[38;2;150;20;170m[DEBUG]\x1b[0m",
            log::Level::Trace => "\x1b[38;2;100;100;100m[TRACE]\x1b[0m",
        };

        if let Some((file, line)) = record.file().zip(record.line()) {
            let file = file.strip_prefix("src/").unwrap_or(file);
            println!("{level}[{file}:{line:>3}] {}", record.args());
        } else {
            println!("{level} {}", record.args());
        }
    }

    fn flush(&self) {
        // do nothing
    }
}

pub fn init() {
    const MAX_LEVEL: log::LevelFilter = if cfg!(debug_assertions) {
        log::LevelFilter::Trace
    } else {
        log::LevelFilter::Info
    };

    unsafe { log::set_logger_racy(&Logger) }.unwrap();
    log::set_max_level(MAX_LEVEL);
}
