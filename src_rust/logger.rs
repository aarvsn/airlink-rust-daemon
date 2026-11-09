use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use chrono::Local;

pub fn init_logger() {
    let logs_dir = Path::new("logs");
    if !logs_dir.exists() {
        let _ = fs::create_dir_all(logs_dir);
    }
}

pub fn log_to_file(level: &str, msg: &str) {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let line = format!("[{}] {}: {}\n", now, level.to_uppercase(), msg);

    let log_file = if level.eq_ignore_ascii_case("error") {
        "logs/error.log"
    } else {
        "logs/combined.log"
    };

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_file) {
        let _ = f.write_all(line.as_bytes());
    }
    if level.eq_ignore_ascii_case("error") {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open("logs/combined.log") {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("\x1b[90m{}\x1b[0m \x1b[34mℹ\x1b[0m \x1b[44m\x1b[1mINFO\x1b[0m \x1b[34m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        $crate::logger::log_to_file("INFO", &msg);
    }};
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("\x1b[90m{}\x1b[0m \x1b[31m✖\x1b[0m \x1b[41m\x1b[1mERROR\x1b[0m \x1b[31m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        $crate::logger::log_to_file("ERROR", &msg);
    }};
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("\x1b[90m{}\x1b[0m \x1b[33m⚠\x1b[0m \x1b[43m\x1b[1mWARN\x1b[0m \x1b[33m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        $crate::logger::log_to_file("WARN", &msg);
    }};
}

#[macro_export]
macro_rules! log_success {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        println!("\x1b[90m{}\x1b[0m \x1b[32m✔\x1b[0m \x1b[42m\x1b[1mSUCCESS\x1b[0m \x1b[32m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
        $crate::logger::log_to_file("SUCCESS", &msg);
    }};
}

#[macro_export]
macro_rules! log_debug {
    ($config:expr, $($arg:tt)*) => {{
        if $config.debug {
            let msg = format!($($arg)*);
            println!("\x1b[90m{}\x1b[0m \x1b[35m⚙\x1b[0m \x1b[45m\x1b[1mDEBUG\x1b[0m \x1b[35m{}\x1b[0m", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
            $crate::logger::log_to_file("DEBUG", &msg);
        }
    }};
}
