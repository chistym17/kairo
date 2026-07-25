pub mod log_entry;

pub use log_entry::{write_entry, write_event, write_run_summary, LogEntry};

use std::sync::atomic::{AtomicBool, Ordering};

/// When true, harness progress lines stay off stdout (JSONL / UI hosts).
static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
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
