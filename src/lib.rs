//! ParamILS — automated algorithm configuration, Rust rewrite.
//!
//! This crate is used in two ways:
//!   1. As a CLI binary (`src/main.rs`) — drop-in for `param_ils_2_3_run.rb`
//!   2. As a Python extension module (feature = "python") — called from Grackle

pub mod params;    // parameter space: domains, defaults, conditionals, forbidden
pub mod scenario;  // scenario file parser (algo, instances, cutoff_time, …)
pub mod eval;      // parallel evaluation engine (rayon thread pool + capping)
pub mod cache;     // persistent result cache (SQLite via rusqlite)
pub mod ils;       // ILS loop: local search, perturbation, acceptance

// ---------------------------------------------------------------------------
// Shared debug helpers
// ---------------------------------------------------------------------------

use std::io::Write;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
// Mutable so it can be opened, closed, and re-opened between Python `specialize()` calls.
static LOG_FILE: LazyLock<Mutex<Option<std::io::LineWriter<std::fs::File>>>> =
    LazyLock::new(|| Mutex::new(None));
/// Set to true when `--debug` is passed — controls stderr output.
static DEBUG_STDERR: AtomicBool = AtomicBool::new(false);

/// Seconds elapsed since the first call to this function (process start).
pub fn t() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Enable stderr debug output (`--debug`).
pub fn enable_debug_stderr() {
    DEBUG_STDERR.store(true, Ordering::Relaxed);
}

/// Open (or replace) the debug log file (`--debug-log` / `specialize(debug_log=…)`).
pub fn init_log_file(path: &str) -> anyhow::Result<()> {
    let file = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("failed to open debug log {path}: {e}"))?;
    *LOG_FILE.lock().unwrap() = Some(std::io::LineWriter::new(file));
    Ok(())
}

/// Close and discard the current debug log file (used after each Python `specialize()` call).
pub fn close_log_file() {
    *LOG_FILE.lock().unwrap() = None;
}

/// Returns true if any debug destination (stderr or file) is active.
pub fn any_debug_active() -> bool {
    DEBUG_STDERR.load(Ordering::Relaxed) || LOG_FILE.lock().unwrap().is_some()
}

/// Write a debug line when `enabled` is true.
///
/// Routing: stderr if `--debug` was set; log file if `--debug-log` was set.
/// `enabled` is the per-category flag (e.g. `options.debug`, `debug_wrapper`).
/// Messages are only emitted when their category flag is on — this ensures
/// wrapper/solver lines don't appear in the log unless those flags are set.
pub fn debug_line(enabled: bool, line: &str) {
    if !enabled { return; }
    if DEBUG_STDERR.load(Ordering::Relaxed) { eprintln!("{line}"); }
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(f) = guard.as_mut() { let _ = writeln!(f, "{line}"); }
    }
}

#[cfg(feature = "python")]
mod python;        // PyO3 bindings — only compiled when building the Python .so
