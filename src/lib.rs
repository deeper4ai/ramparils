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
// Holds the path, not an open file: the error log is created on the first
// crash, not at startup, so a run with nothing to report never leaves a
// zero-byte file behind (see `log_crash`).
static ERROR_LOG: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
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

/// Die on a closed stdout instead of panicking, as a Unix tool should.
///
/// Rust ignores `SIGPIPE` at startup so that a write to a closed pipe surfaces
/// as an `EPIPE` error; `println!` then panics on it, which turns
/// `ramparils db cache | head` into a backtrace. Restoring the default
/// disposition makes the process exit quietly at the point the reader goes
/// away, which is what every other command-line tool does.
///
/// Called once from `main`, before any output.
pub fn restore_default_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
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

/// Open the debug log file (`--debug-log` / `specialize(debug_log=…)`), appending.
///
/// A rerun against the same path keeps prior runs' output rather than
/// truncating it, so the file is a running history across invocations, not
/// just this one.
pub fn init_log_file(path: &str) -> anyhow::Result<()> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open debug log {path}: {e}"))?;
    *LOG_FILE.lock().unwrap() = Some(std::io::LineWriter::new(file));
    Ok(())
}

/// Close and discard the current debug log file (used after each Python `specialize()` call).
pub fn close_log_file() {
    *LOG_FILE.lock().unwrap() = None;
}

/// Register the error log path (`--error-log` / `specialize(error_log=…)`).
///
/// Validates the path is writable now — so a bad path fails fast at startup,
/// as before. Two rules balance against each other:
///
/// - A path with no file yet is left with none: the log is created for real
///   only on the first crash (`log_crash`), so a clean run never produces a
///   zero-byte error log that looks like evidence nothing was checked.
/// - A path that already holds crash history (from an earlier run against
///   the same scenario) keeps it: this call never truncates or deletes an
///   existing file, only appends to it later, so a rerun's crashes add to
///   the running history instead of erasing the previous run's.
pub fn init_error_log(path: &str) -> anyhow::Result<()> {
    let existed = std::path::Path::new(path).exists();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| anyhow::anyhow!("failed to open error log {path}: {e}"))?;
    drop(file);
    if !existed {
        // Only remove what this call just created -- pre-existing history
        // (and, if concurrently written, any content it just gained) is
        // never touched.
        let _ = std::fs::remove_file(path);
    }
    *ERROR_LOG.lock().unwrap() = Some(path.to_string());
    Ok(())
}

/// Forget the current error log path.
pub fn close_error_log() {
    *ERROR_LOG.lock().unwrap() = None;
}

/// Write a crash entry to the error log (if one is registered), creating the
/// file on the first call and appending on every one after.
///
/// Called from worker threads — the `Mutex` serialises concurrent writes so
/// entries from parallel solver calls never interleave.
pub fn log_crash(cmd: &str, stdout: &str, stderr: &str, exit_code: Option<i32>) {
    let guard = ERROR_LOG.lock().unwrap();
    let Some(path) = guard.as_deref() else { return };
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return };
    let mut f = std::io::LineWriter::new(file);
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

/// Probe `algo`'s `--version` response before any evaluation runs.
///
/// Runs `sh -c "<algo> --version"` — the same path `run_wrapper_process`
/// (`eval.rs`) takes for a real evaluation, so this also proves the `algo`
/// string is executable at all, before the budget is spent on it
/// (ramparils-primo-cont/IDEAS.md item 1).
///
/// Requires: the process exits 0, its stdout's **last** line starts with
/// `supports:`, and that line lists `version` as a whitespace-separated
/// token. Anything else is an error — "No valid response, no run." Returns
/// the trimmed stdout on success, for the caller to log.
///
/// The incident this closes: a 24 h run against a wrapper with no solver
/// binary on `PATH` and no instance files in place reported nothing wrong,
/// because the wrapper answered every evaluation with a well-formed but
/// meaningless result line rather than crashing (`TODO.md`, 2026-08-20). This
/// probe would have refused to start in under a second.
pub fn probe_wrapper_version(algo: &str) -> anyhow::Result<String> {
    let output = std::process::Command::new("sh")
        .args(["-c", &format!("{algo} --version")])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run `{algo} --version`: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim_end().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim_end().to_string();
    let report = |detail: &str| -> anyhow::Error {
        anyhow::anyhow!(
            "`{algo} --version` {detail}; refusing to start.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        )
    };

    if !output.status.success() {
        return Err(report(&format!("exited with {}", output.status)));
    }
    let Some(last_line) = stdout.lines().next_back() else {
        return Err(report("produced no output"));
    };
    let Some(supports) = last_line.strip_prefix("supports:") else {
        return Err(report("did not end with a `supports:` line"));
    };
    if !supports.split_whitespace().any(|feature| feature == "version") {
        return Err(report("`supports:` line does not list `version`"));
    }

    Ok(stdout)
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

    /// A clean run (nothing crashes) must not leave a zero-byte error log
    /// behind — only `init_error_log`'s own writability probe touches the
    /// path, and that probe removes what it created.
    #[test]
    fn error_log_not_created_until_first_crash() {
        let path = std::env::temp_dir().join(format!("ramparils-test-errlog-{}", std::process::id()));
        let path_str = path.to_str().unwrap();

        super::init_error_log(path_str).unwrap();
        assert!(!path.exists(), "init_error_log must not leave the file behind");

        super::log_crash("cmd", "out", "err", Some(1));
        assert!(path.exists(), "log_crash must create the file on first write");
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("cmd"));

        super::log_crash("cmd2", "", "", None);
        let second = std::fs::read_to_string(&path).unwrap();
        assert!(
            second.starts_with(&first),
            "a later crash must append, not truncate, the earlier entry"
        );
        assert!(second.contains("cmd2"));
        super::close_error_log();

        // A rerun against the same path (a fresh `ramparils run` invocation,
        // not just a fresh crash within one -- kept in this same test, not a
        // separate one, since `ERROR_LOG` is a shared global static and a
        // second test running in parallel on it would race this one) must
        // keep the previous run's crash history rather than reset it.
        super::init_error_log(path_str).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            second,
            "re-registering the path must not truncate the previous run's history"
        );
        super::log_crash("cmd3", "out3", "err3", Some(1));
        let third = std::fs::read_to_string(&path).unwrap();
        assert!(third.starts_with(&second), "a later run must append after the earlier run's entries, not replace them");
        assert!(third.contains("cmd3"));

        super::close_error_log();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn probe_wrapper_version_ok() {
        let block = super::probe_wrapper_version(
            "printf 'primo_wrapper.py 0.1.0\\nprimo 0.1.0\\nsupports: version runhash params'",
        )
        .unwrap();
        assert!(block.contains("supports: version runhash params"));
    }

    #[test]
    fn probe_wrapper_version_rejects_nonzero_exit() {
        let err = super::probe_wrapper_version("sh -c 'echo supports: version; exit 1'")
            .unwrap_err()
            .to_string();
        assert!(err.contains("exited with"), "{err}");
    }

    #[test]
    fn probe_wrapper_version_rejects_missing_supports_line() {
        let err = super::probe_wrapper_version("echo no version info here")
            .unwrap_err()
            .to_string();
        assert!(err.contains("supports:"), "{err}");
    }

    #[test]
    fn probe_wrapper_version_rejects_supports_without_version() {
        let err = super::probe_wrapper_version("echo supports: runhash params")
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not list"), "{err}");
    }
}
