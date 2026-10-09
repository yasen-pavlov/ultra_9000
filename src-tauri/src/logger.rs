//! Minimal stderr logger for native diagnostics.
//!
//! Level from `ULTRA9000_LOG` (`error`, `warn`, `info`, `debug`, `trace`), default `warn`.
//! `debug` also enables Alacritty's OpenGL debug output, exactly as its own `-v` flags do.

use std::io::Write;

use log::{LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = write_record(&mut std::io::stderr(), record);
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

/// One line per record: `[LEVEL target] message`.
fn write_record(out: &mut impl Write, record: &Record<'_>) -> std::io::Result<()> {
    writeln!(
        out,
        "[{} {}] {}",
        record.level(),
        record.target(),
        record.args()
    )
}

/// The configured level; unset or unrecognized values fall back to `warn`.
fn level(setting: Option<&str>) -> LevelFilter {
    setting
        .and_then(|level| level.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Warn)
}

static LOGGER: StderrLogger = StderrLogger;

pub fn init() {
    let level = level(std::env::var("ULTRA9000_LOG").ok().as_deref());

    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests;
