//! ParamILS — automated algorithm configuration, Rust rewrite.
//!
//! This crate is used in two ways:
//!   1. As a CLI binary (`src/main.rs`) — drop-in for `param_ils_2_3_run.rb`
//!   2. As a Python extension module (feature = "python") — called from Grackle

pub mod cache; // persistent result cache (SQLite via rusqlite)
pub mod db; // read-only export of a .dbcache (the `ramparils db` sub-commands)
pub mod eval; // parallel evaluation engine (rayon thread pool + capping)
pub mod ils;
pub mod params; // parameter space: domains, defaults, conditionals, forbidden
pub mod scenario; // scenario file parser (algo, instances, cutoff_time, …) // ILS loop: local search, perturbation, acceptance

/// Source revision this binary was built from, suffixed `-dirty` when the
/// worktree had uncommitted changes, or `"unknown"` when it was built without
/// git (an sdist, a source tarball). Set by `build.rs`.
///
/// `CARGO_PKG_VERSION` is not a substitute: it only moves on release, so every
/// commit between two tags reports the same version. A tuning run's log is
/// usually the only lasting record of what produced a result, which makes this
/// the difference between a run that can be reproduced and one that merely
/// claims a version number.
pub const GIT_REVISION: &str = env!("RAMPARILS_GIT");

/// Build profile, compiler version and target triple. Set by `build.rs`.
pub const BUILD_INFO: &str = env!("RAMPARILS_BUILD");

// ---------------------------------------------------------------------------
// Shared debug helpers
// ---------------------------------------------------------------------------

use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
// Mutable so it can be opened, closed, and re-opened between Python `specialize()` calls.
static LOG_FILE: LazyLock<Mutex<Option<std::io::LineWriter<std::fs::File>>>> = LazyLock::new(|| Mutex::new(None));
static ERROR_LOG: LazyLock<Mutex<Option<std::io::LineWriter<std::fs::File>>>> = LazyLock::new(|| Mutex::new(None));
/// Set to true when `--debug` is passed — controls stderr output.
static DEBUG_STDERR: AtomicBool = AtomicBool::new(false);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static ACTIVE_PROCESS_GROUPS: LazyLock<Mutex<HashSet<libc::pid_t>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

extern "C" fn handle_termination_signal(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Install CLI termination handlers. The handler only sets an atomic flag;
/// worker threads perform process cleanup outside signal context.
pub fn install_signal_handlers() -> anyhow::Result<()> {
    unsafe {
        if libc::signal(
            libc::SIGINT,
            handle_termination_signal as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
        {
            return Err(anyhow::anyhow!("failed to install SIGINT handler"));
        }
        if libc::signal(
            libc::SIGTERM,
            handle_termination_signal as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
        {
            return Err(anyhow::anyhow!("failed to install SIGTERM handler"));
        }
    }
    Ok(())
}

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

pub(crate) fn register_process_group(process_group: libc::pid_t) {
    ACTIVE_PROCESS_GROUPS.lock().unwrap().insert(process_group);
}

pub(crate) fn unregister_process_group(process_group: libc::pid_t) {
    ACTIVE_PROCESS_GROUPS.lock().unwrap().remove(&process_group);
}

pub fn terminate_active_process_groups() {
    let groups: Vec<libc::pid_t> = ACTIVE_PROCESS_GROUPS.lock().unwrap().iter().copied().collect();
    for process_group in &groups {
        unsafe {
            libc::kill(-process_group, libc::SIGTERM);
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
    for process_group in groups {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
}

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
    let file = std::fs::File::create(path).map_err(|e| anyhow::anyhow!("failed to open debug log {path}: {e}"))?;
    *LOG_FILE.lock().unwrap() = Some(std::io::LineWriter::new(file));
    Ok(())
}

/// Close and discard the current debug log file (used after each Python `specialize()` call).
pub fn close_log_file() {
    *LOG_FILE.lock().unwrap() = None;
}

/// Open (or replace) the error log file (`--error-log` / `specialize(error_log=…)`).
pub fn init_error_log(path: &str) -> anyhow::Result<()> {
    let file = std::fs::File::create(path).map_err(|e| anyhow::anyhow!("failed to open error log {path}: {e}"))?;
    *ERROR_LOG.lock().unwrap() = Some(std::io::LineWriter::new(file));
    Ok(())
}

/// Close and discard the current error log file.
pub fn close_error_log() {
    *ERROR_LOG.lock().unwrap() = None;
}

/// Write a crash entry to the error log (if one is open).
///
/// Called from worker threads — the `Mutex` serialises concurrent writes so
/// entries from parallel solver calls never interleave.
pub fn log_crash(cmd: &str, stdout: &str, stderr: &str, exit_code: Option<i32>) {
    let mut guard = ERROR_LOG.lock().unwrap();
    let Some(f) = guard.as_mut() else { return };
    let t = t();
    let exit_str = exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "spawn_failed".to_string());
    let sep = "=".repeat(60);
    let _ = writeln!(f, "{sep}");
    let _ = writeln!(f, "crash  t={t:.2}s  exit={exit_str}");
    let _ = writeln!(f, "cmd: {cmd}");
    if !stdout.is_empty() {
        let _ = writeln!(f, "--- stdout ---");
        let _ = write!(f, "{stdout}");
        if !stdout.ends_with('\n') {
            let _ = writeln!(f);
        }
    }
    if !stderr.is_empty() {
        let _ = writeln!(f, "--- stderr ---");
        let _ = write!(f, "{stderr}");
        if !stderr.ends_with('\n') {
            let _ = writeln!(f);
        }
    }
    let _ = writeln!(f, "{sep}");
}

/// Returns true if any debug destination (stderr or file) is active.
pub fn any_debug_active() -> bool {
    DEBUG_STDERR.load(Ordering::Relaxed) || LOG_FILE.lock().unwrap().is_some()
}

/// Debug output settings — groups all debug flags so they can be passed as one argument.
#[derive(Clone, Copy, Debug, Default)]
pub struct DebugOptions {
    /// General ILS output: new incumbents, BLS steps, perturbations. (`--debug`)
    pub main: bool,
    /// One line per solver wrapper invocation. (`--debug-wrapper`)
    pub wrapper: bool,
    /// One line per solver result. (`--debug-solver`)
    pub solver: bool,
}

/// Write a debug line when `enabled` is true.
///
/// Routing: stderr if `--debug` was set; log file if `--debug-log` was set.
/// `enabled` is the per-category flag (e.g. `options.debug`, `debug_wrapper`).
/// Messages are only emitted when their category flag is on — this ensures
/// wrapper/solver lines don't appear in the log unless those flags are set.
pub fn debug_line(enabled: bool, line: &str) {
    if !enabled {
        return;
    }
    if DEBUG_STDERR.load(Ordering::Relaxed) {
        eprintln!("{line}");
    }
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(f) = guard.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Write multiline debug output through the configured debug destinations.
pub fn debug_block(enabled: bool, block: &str) {
    if !enabled {
        return;
    }
    for line in block.lines() {
        debug_line(true, line);
    }
}

#[cfg(feature = "python")]
mod python; // PyO3 bindings — only compiled when building the Python .so

#[cfg(test)]
mod tests {
    /// The build script must always produce these, including where git is
    /// unavailable. A silent failure would leave the header claiming an empty
    /// revision, which reads as a formatting bug rather than as missing
    /// provenance.
    #[test]
    fn build_stamps_are_populated() {
        assert!(!super::GIT_REVISION.is_empty());
        assert!(
            !super::GIT_REVISION.contains(char::is_whitespace),
            "revision should be a bare sha, got {:?}",
            super::GIT_REVISION
        );
        assert!(!super::BUILD_INFO.is_empty());
    }
}
