//! Sends the tray app's own `log` messages to server.log, next to the
//! server's output, so a failed menu action leaves a trace.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

struct FileLogger {
    path: PathBuf,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        // Only this app's messages, not those of Tauri and its plugins.
        if !self.enabled(record.metadata())
            || !record.target().starts_with(env!("CARGO_CRATE_NAME"))
        {
            return;
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(
                file,
                "[{}] tray {}: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level().as_str().to_lowercase(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

pub(crate) fn init(path: PathBuf) {
    // Leaked once at startup; the logger lives as long as the app.
    if log::set_logger(Box::leak(Box::new(FileLogger { path }))).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}
