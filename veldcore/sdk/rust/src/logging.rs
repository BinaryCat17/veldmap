use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};

pub const FLAG_PERF: u32 = 1 << 0;

#[macro_export]
macro_rules! vinfo {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Info, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Info, $($arg)+)
    };
}

#[macro_export]
macro_rules! vwarn {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Warn, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Warn, $($arg)+)
    };
}

#[macro_export]
macro_rules! verror {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Error, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Error, $($arg)+)
    };
}

#[macro_export]
macro_rules! vdebug {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Debug, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Debug, $($arg)+)
    };
}

#[macro_export]
macro_rules! vtrace {
    ($flags:expr, $($arg:tt)+) => {
        $crate::log::log!(target: &format!("veldmap_vlog:{}", $flags), $crate::log::Level::Trace, $($arg)+)
    };
    ($($arg:tt)+) => {
        $crate::log::log!($crate::log::Level::Trace, $($arg)+)
    };
}

pub struct HostLogger;
impl Log for HostLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool { true }
    fn log(&self, record: &Record) {
        let mut flags = 0;
        let target = record.target();
        if let Some(flags_str) = target.strip_prefix("veldmap_vlog:") {
            flags = flags_str.parse::<u32>().unwrap_or(0);
        }

        // Прямой ABI-вызов: без RPC-хопа через диспетчер и system-сервис.
        crate::rpc::host::log(record.level(), flags, &format!("{}", record.args()));
    }
    fn flush(&self) {}
}

pub fn init() -> Result<(), SetLoggerError> {
    log::set_logger(&HostLogger).map(|_| log::set_max_level(LevelFilter::Info))
}
