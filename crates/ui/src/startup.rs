//! Startup timing instrumentation (Phase 09 quality gates): the process
//! start instant is marked by the binary before the GPUI runtime starts;
//! the first main-window render notes the elapsed time once and logs it.
//! No values, no secrets — one duration, logged like any other metric.

use std::sync::OnceLock;
use std::time::Instant;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static FIRST_FRAME: OnceLock<Instant> = OnceLock::new();

/// Called by the binary before the application runtime starts.
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// Called by the main window's render (cheap `OnceLock` check). Logs the
/// process-start → first-frame duration exactly once.
pub fn note_first_frame() {
    if FIRST_FRAME.get().is_some() {
        return;
    }
    let Some(start) = PROCESS_START.get() else {
        return;
    };
    if FIRST_FRAME.set(Instant::now()).is_ok() {
        let elapsed = FIRST_FRAME
            .get()
            .expect("just set")
            .duration_since(*start)
            .as_millis();
        licoman_core::log::info(
            "licoman::startup",
            "first frame rendered",
            &[("elapsed_ms", elapsed.to_string())],
        );
    }
}

/// The recorded (start, first-frame) instants, for tests and diagnostics.
pub fn measured() -> Option<(Instant, Instant)> {
    Some((*PROCESS_START.get()?, *FIRST_FRAME.get()?))
}
