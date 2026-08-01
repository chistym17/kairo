pub mod log_entry;

pub use log_entry::{
    begin_run_log, emit_thinking_state, logs_dir, next_run_sequence, run_log_path,
    set_logs_dir_override, write_entry, write_event, write_run_summary, LogEntry,
};

use std::sync::atomic::{AtomicBool, Ordering};

/// When true, harness progress lines stay off stdout (product CLI default).
static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// True when `KAIRO_VERBOSE` is 1/true/yes/on (case-insensitive).
pub fn env_verbose() -> bool {
    match std::env::var("KAIRO_VERBOSE") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

/// Apply product CLI quiet policy: quiet unless `--verbose` or `KAIRO_VERBOSE`.
pub fn configure_cli_quiet(verbose_flag: bool) {
    set_quiet(!(verbose_flag || env_verbose()));
}

/// Progress line for humans. No-op when [`set_quiet`] is true.
#[macro_export]
macro_rules! agent_println {
    ($($arg:tt)*) => {{
        if !$crate::logging::is_quiet() {
            println!($($arg)*);
        }
    }};
}
