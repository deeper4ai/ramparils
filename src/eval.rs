//! Parallel evaluation scheduler.
//!
//! Parallelism is over `(neighbor, instance)` pairs — works even with a single
//! instance because we still have N neighbors × 1 instance = N parallel calls.
//!
//! # Flow
//!
//! ```text
//! ILS                            Scheduler                 Workers (×N)
//!  │                                │                          │
//!  │── submit(tasks, cache) ───────▶│                          │
//!  │                          bulk cache read                  │
//!  │                          hits ──────────────────▶ result_rx
//!  │                          misses ────────────────────────▶ work batches
//!  │                                │                          │── run_solver ──▶
//!  │◀── results().recv() ──────────────────────────────────────│
//!  │                                │                          │
//!  │── reset() ────────────────────▶│  (batch invalidated)     │
//!  │   drain result_rx              │                          │ (finish call,
//!  │                                │                          │  skip pending)
//! ```
//!
//! # Cache write-back
//!
//! `TaskResult` carries the `instance_id` and `hash` needed to call
//! `cache.put()`.  The ILS writes back each result as it reads it from the
//! result channel — keeps cache access single-threaded in the ILS.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossbeam::channel::{Receiver, Sender, unbounded};

use crate::cache::{Cache, CachedResult};
use crate::params::Config;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One evaluation task — one neighbor config evaluated on all instances.
pub struct EvalTask {
    pub neighbor_id: usize,
    pub config: Config,
    /// Pre-computed hash of `config` (for cache lookup and write-back).
    pub hash: u64,
    /// `(instance_id, instance_path)` pairs — id for cache, path for solver.
    pub instances: Arc<Vec<(i64, String)>>,
}

/// Result of one `(neighbor, instance)` evaluation.
pub struct TaskResult {
    pub batch_id: u64,
    pub neighbor_id: usize,
    pub instance_id: i64,
    /// Hash of the config (for cache write-back).
    pub hash: u64,
    pub runtime: f64,
    pub quality: f64,
    pub status: String,
    /// Fingerprint of the solver's internal behaviour on this instance;
    /// `None` for anything but a terminated (sat/unsat-shaped) run.
    pub runhash: Option<u64>,
    /// Only results produced by a solver execution may be persisted.
    pub cacheable: bool,
    pub cutoff: f64,
}

/// Aggregated result for one config across all instances.
#[derive(Debug, Clone)]
pub struct EvalResult {
    pub runtimes: Vec<f64>,
    pub pruned: bool,
}

impl EvalResult {
    pub fn mean(&self) -> f64 {
        self.runtimes.iter().sum::<f64>() / self.runtimes.len() as f64
    }

    pub fn median(&self) -> f64 {
        let mut s = self.runtimes.clone();
        s.sort_by(f64::total_cmp);
        let n = s.len();
        if n % 2 == 0 {
            (s[n / 2 - 1] + s[n / 2]) / 2.0
        } else {
            s[n / 2]
        }
    }
}

// ---------------------------------------------------------------------------
// Internal types (worker-side)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct WorkBatch {
    batch_id: u64,
    neighbor_id: usize,
    config: Arc<Config>,
    hash: u64,
    instances: Arc<Vec<(i64, String)>>,
    missing_indices: Arc<Vec<usize>>,
    next_index: Arc<AtomicUsize>,
    cutoff_time: f64,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

pub struct Scheduler {
    work_tx: Sender<WorkBatch>,
    result_tx: Sender<TaskResult>, // used in submit() for cache hits
    result_rx: Receiver<TaskResult>,
    batch_id: Arc<AtomicU64>,
    n_workers: usize,
    #[cfg(test)]
    submitted_work_batches: AtomicUsize,
    cutoff_time: f64,
    debug: crate::DebugOptions,
    active_process_groups: Arc<Mutex<HashMap<libc::pid_t, u64>>>,
    _workers: Vec<std::thread::JoinHandle<()>>,
}

impl Scheduler {
    /// Spawn `n_workers` worker threads. Each loops waiting for work batches,
    /// runs the solver, and sends `TaskResult`s back.
    pub fn new(n_workers: usize, algo: String, cutoff_time: f64, debug: crate::DebugOptions) -> Self {
        let (work_tx, work_rx) = unbounded::<WorkBatch>();
        let (result_tx, result_rx) = unbounded::<TaskResult>();
        let batch_id = Arc::new(AtomicU64::new(0));
        let active_process_groups = Arc::new(Mutex::new(HashMap::new()));

        let workers = (0..n_workers)
            .map(|_| {
                let work_rx: Receiver<WorkBatch> = work_rx.clone();
                let result_tx: Sender<TaskResult> = result_tx.clone();
                let current_batch = Arc::clone(&batch_id);
                let active_process_groups = Arc::clone(&active_process_groups);
                let algo = algo.clone();

                std::thread::spawn(move || {
                    while let Ok(batch) = work_rx.recv() {
                        if batch.batch_id != current_batch.load(Ordering::Relaxed) {
                            continue;
                        }
                        loop {
                            let index = batch.next_index.fetch_add(1, Ordering::Relaxed);
                            let Some(&instance_index) = batch.missing_indices.get(index) else {
                                break;
                            };
                            if batch.batch_id != current_batch.load(Ordering::Relaxed) {
                                break;
                            }
                            let (instance_id, instance_path) = &batch.instances[instance_index];
                            let Some((runtime, quality, status, runhash)) = run_solver_inner(
                                &algo,
                                &batch.config,
                                batch.hash,
                                instance_path,
                                batch.cutoff_time,
                                batch.batch_id,
                                &current_batch,
                                debug,
                                &active_process_groups,
                            ) else {
                                break;
                            };
                            let _ = result_tx.send(TaskResult {
                                batch_id: batch.batch_id,
                                neighbor_id: batch.neighbor_id,
                                instance_id: *instance_id,
                                hash: batch.hash,
                                runtime,
                                quality,
                                status,
                                runhash,
                                cacheable: true,
                                cutoff: batch.cutoff_time,
                            });
                        }
                    }
                })
            })
            .collect();

        crate::debug_line(
            debug.main,
            &format!("[{:8.2}s] eval: scheduler started workers={n_workers}", crate::t()),
        );
        Scheduler {
            work_tx,
            result_tx,
            result_rx,
            batch_id,
            n_workers,
            #[cfg(test)]
            submitted_work_batches: AtomicUsize::new(0),
            cutoff_time,
            debug,
            active_process_groups,
            _workers: workers,
        }
    }

    /// Dispatch one batch of tasks.
    ///
    /// For each task, issues one bulk cache query; hits go directly to the
    /// result channel, while misses are grouped into shared work batches.
    ///
    /// Returns an id that identifies all results from this submission.
    pub fn submit(&self, tasks: Vec<EvalTask>, cache: &Cache) -> Result<u64> {
        let batch_id = self.batch_id.fetch_add(1, Ordering::Relaxed) + 1;

        let (mut n_hits, mut n_misses) = (0usize, 0usize);
        #[cfg(test)]
        let mut n_work_batches = 0usize;
        for task in tasks {
            // Every configuration the tuner ever evaluates passes through here,
            // so this is where the cache learns what its hashes mean.
            cache.put_strategy(task.hash, &task.config)?;

            let ids: Vec<i64> = task.instances.iter().map(|(id, _)| *id).collect();
            let hits: HashMap<i64, CachedResult> = cache.get_batch(task.hash, &ids, self.cutoff_time)?;
            let mut missing_indices = Vec::with_capacity(task.instances.len() - hits.len());

            for (index, (instance_id, _)) in task.instances.iter().enumerate() {
                if let Some(cached) = hits.get(instance_id) {
                    n_hits += 1;
                    let _ = self.result_tx.send(TaskResult {
                        batch_id,
                        neighbor_id: task.neighbor_id,
                        instance_id: *instance_id,
                        hash: task.hash,
                        runtime: cached.runtime,
                        quality: cached.quality,
                        status: cached.status.clone(),
                        runhash: cached.runhash,
                        cacheable: false,
                        cutoff: self.cutoff_time,
                    });
                } else {
                    n_misses += 1;
                    missing_indices.push(index);
                }
            }

            if !missing_indices.is_empty() {
                let worker_slots = self.n_workers.min(missing_indices.len());
                let batch = WorkBatch {
                    batch_id,
                    neighbor_id: task.neighbor_id,
                    config: Arc::new(task.config),
                    hash: task.hash,
                    instances: task.instances,
                    missing_indices: Arc::new(missing_indices),
                    next_index: Arc::new(AtomicUsize::new(0)),
                    cutoff_time: self.cutoff_time,
                };
                for _ in 0..worker_slots {
                    let _ = self.work_tx.send(batch.clone());
                    #[cfg(test)]
                    {
                        n_work_batches += 1;
                    }
                }
            }
        }
        #[cfg(test)]
        self.submitted_work_batches.store(n_work_batches, Ordering::Relaxed);
        crate::debug_line(
            self.debug.main,
            &format!(
                "[{:8.2}s] eval: submitted tasks={} hits={n_hits} misses={n_misses}",
                crate::t(),
                n_hits + n_misses
            ),
        );
        Ok(batch_id)
    }

    /// The result channel — ILS reads `TaskResult`s from here as they arrive.
    pub fn results(&self) -> &Receiver<TaskResult> {
        &self.result_rx
    }

    #[cfg(test)]
    fn submitted_work_batches(&self) -> usize {
        self.submitted_work_batches.load(Ordering::Relaxed)
    }

    /// Invalidate the current batch, terminate its active solver processes,
    /// and make workers skip its queued items.
    ///
    /// The ILS should drain `results()` after calling this to consume any
    /// results completed just before cancellation.
    pub fn reset(&self) {
        let invalidated_batch = self.batch_id.fetch_add(1, Ordering::Relaxed);
        terminate_batch_process_groups(&self.active_process_groups, invalidated_batch);
        crate::debug_line(self.debug.main, &format!("[{:8.2}s] eval: reset", crate::t()));
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.reset();
        terminate_process_groups(&self.active_process_groups);
    }
}

// ---------------------------------------------------------------------------
// Solver subprocess
// ---------------------------------------------------------------------------

/// Quality penalty returned for failed/interrupted runs (no result line).
/// Matches the grackle wrapper convention: penalty=10_000_000 for unsolved.
const UNKNOWN_QUALITY: f64 = 10_000_000.0;

/// Invoke the solver and return `(runtime, quality, status, runhash)`.
///
/// Command format:
/// ```text
/// algo  instance  cutoff_time  -key val ...
/// ```
/// The solver must print a result line to stdout:
/// ```text
/// #%# RamParIls #%# <status>, <runtime>, <quality>[, <runhash>]
/// ```
/// where `<status>` is the raw solver status string (e.g. `Theorem`,
/// `Timeout`, `sat`, `CRASHED`) and the optional `<runhash>` is a hex
/// fingerprint of the solver's internal behaviour (IDEAS.md item 2) —
/// present only when the run actually terminated.
///
/// On crash or missing result line, returns `(cutoff_time, 0.0, "UNKNOWN", None)`.
///
/// # Security note
/// The algo string is passed to `sh -c` for shell compatibility.
/// Only trusted scenario files should be used.
#[allow(clippy::too_many_arguments)]
fn run_solver_inner(
    algo: &str,
    config: &Config,
    hash: u64,
    instance: &str,
    cutoff_time: f64,
    batch_id: u64,
    current_batch: &AtomicU64,
    debug: crate::DebugOptions,
    active_process_groups: &Arc<Mutex<HashMap<libc::pid_t, u64>>>,
) -> Option<(f64, f64, String, Option<u64>)> {
    let mut pairs: Vec<(&String, &String)> = config.iter().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);
    let paramstring = pairs
        .iter()
        .map(|(k, v)| format!("-{k} {v}"))
        .collect::<Vec<_>>()
        .join(" ");

    let cmd = format!("{algo} {instance} {cutoff_time} {paramstring}");
    crate::debug_line(debug.wrapper, &format!("[{:8.2}s] wrapper: {cmd}", crate::t()));

    let output = run_wrapper_process(&cmd, batch_id, current_batch, active_process_groups);

    let (runtime, quality, status, runhash, result_line) = match output {
        Ok(Some(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let (rt, q, st, rh, rl) = parse_solver_output(&stdout, cutoff_time);
            if st == "UNKNOWN" {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::log_crash(&cmd, &stdout, &stderr, out.status.code());
            }
            (rt, q, st, rh, rl)
        }
        Ok(None) => return None,
        Err(e) => {
            crate::log_crash(&cmd, "", &e.to_string(), None);
            (
                cutoff_time,
                UNKNOWN_QUALITY,
                "UNKNOWN".to_string(),
                None,
                format!("#%# RamParIls #%# UNKNOWN, {cutoff_time}, {UNKNOWN_QUALITY}"),
            )
        }
    };
    let iname = std::path::Path::new(instance)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(instance);
    crate::debug_line(
        debug.solver,
        &format!("[{:8.2}s] solver: {hash:016x} @ {iname} {result_line}", crate::t()),
    );
    Some((runtime, quality, status, runhash))
}

fn run_wrapper_process(
    cmd: &str,
    batch_id: u64,
    current_batch: &AtomicU64,
    active_process_groups: &Arc<Mutex<HashMap<libc::pid_t, u64>>>,
) -> std::io::Result<Option<std::process::Output>> {
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let process_group = child.id() as libc::pid_t;
    crate::register_process_group(process_group);
    active_process_groups.lock().unwrap().insert(process_group, batch_id);
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let status = loop {
        let canceled = crate::interrupted() || batch_id != current_batch.load(Ordering::Relaxed);
        if canceled {
            terminate_child_process_group(&mut child, process_group)?;
            break None;
        }
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    crate::unregister_process_group(process_group);
    active_process_groups.lock().unwrap().remove(&process_group);
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("stdout reader thread panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("stderr reader thread panicked"))??;
    Ok(status.map(|status| std::process::Output { status, stdout, stderr }))
}

fn terminate_child_process_group(child: &mut std::process::Child, process_group: libc::pid_t) -> std::io::Result<()> {
    unsafe {
        libc::kill(-process_group, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(100));
    if child.try_wait()?.is_none() {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    child.wait()?;
    Ok(())
}

fn terminate_batch_process_groups(active_process_groups: &Mutex<HashMap<libc::pid_t, u64>>, batch_id: u64) {
    let groups: Vec<libc::pid_t> = active_process_groups
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(&process_group, &active_batch)| (active_batch == batch_id).then_some(process_group))
        .collect();
    terminate_groups(&groups);
}

fn terminate_process_groups(active_process_groups: &Mutex<HashMap<libc::pid_t, u64>>) {
    let groups: Vec<libc::pid_t> = active_process_groups.lock().unwrap().keys().copied().collect();
    terminate_groups(&groups);
}

fn terminate_groups(groups: &[libc::pid_t]) {
    for &process_group in groups {
        unsafe {
            libc::kill(-process_group, libc::SIGTERM);
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    for &process_group in groups {
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
}

fn parse_solver_output(output: &str, cutoff_time: f64) -> (f64, f64, String, Option<u64>, String) {
    for line in output.lines() {
        let rest = match line.strip_prefix("#%# RamParIls #%# ") {
            Some(r) => r,
            None => continue,
        };

        // Format: status, runtime, quality[, runhash]. The optional fourth
        // field (IDEAS.md item 2) must stay outside quality's split segment,
        // or a wrapper that emits it turns every evaluation into UNKNOWN at
        // cutoff: splitn(3, ',') would leave "<quality>, <runhash>" in the
        // quality segment and parse::<f64>() on that fails silently.
        let mut parts = rest.splitn(4, ',');
        let status = parts.next().map(|s| s.trim().to_string());
        let runtime = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
        let quality = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
        // The runhash is a hex fingerprint of the solver's internal work on
        // this instance (`RunHash` in solverpy), present only for a run that
        // actually terminated -- absent (and left `None`) on a malformed or
        // missing field, which happens for every timeout/error/unknown status
        // by the wrapper's own contract.
        let runhash = parts.next().and_then(|s| u64::from_str_radix(s.trim(), 16).ok());

        if let (Some(st), Some(rt), Some(q)) = (status, runtime, quality) {
            return (rt.min(cutoff_time), q, st, runhash, line.to_string());
        }
    }
    let fallback = format!("#%# RamParIls #%# UNKNOWN, {cutoff_time}, {UNKNOWN_QUALITY}");
    (cutoff_time, UNKNOWN_QUALITY, "UNKNOWN".to_string(), None, fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Cache, hash_config};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    #[test]
    fn parse_ok_line() {
        let (rt, q, st, rh, raw) =
            parse_solver_output("some preamble\n#%# RamParIls #%# Theorem, 1.23, 42.0\n", 10.0);
        assert!((rt - 1.23).abs() < 1e-9);
        assert!((q - 42.0).abs() < 1e-9);
        assert_eq!(st, "Theorem");
        assert_eq!(rh, None);
        assert!(raw.contains("Theorem"));
    }

    #[test]
    fn parse_timeout_line() {
        let (rt, _, st, rh, _) = parse_solver_output("#%# RamParIls #%# Timeout, 5.0, 0.0", 10.0);
        assert!((rt - 5.0).abs() < 1e-9);
        assert_eq!(st, "Timeout");
        assert_eq!(rh, None);
    }

    #[test]
    fn parse_ok_line_with_runhash() {
        let (rt, q, st, rh, raw) =
            parse_solver_output("#%# RamParIls #%# sat, 1.23, 0.0, 3eff6fcf0e4d910d\n", 10.0);
        assert!((rt - 1.23).abs() < 1e-9);
        assert!((q - 0.0).abs() < 1e-9);
        assert_eq!(st, "sat");
        assert_eq!(rh, Some(0x3eff6fcf0e4d910d));
        assert!(raw.contains("3eff6fcf0e4d910d"));
    }

    #[test]
    fn parse_caps_at_cutoff() {
        let (rt, _, _, _, _) = parse_solver_output("#%# RamParIls #%# Theorem, 99.9, 0.0", 10.0);
        assert!((rt - 10.0).abs() < 1e-9);
    }

    #[test]
    fn parse_missing_returns_cutoff() {
        let (rt, q, st, rh, raw) = parse_solver_output("no result here", 5.0);
        assert!((rt - 5.0).abs() < 1e-9);
        assert!((q - UNKNOWN_QUALITY).abs() < 1e-9);
        assert_eq!(st, "UNKNOWN");
        assert_eq!(rh, None);
        assert!(raw.contains("UNKNOWN"));
    }

    #[test]
    fn scheduler_cache_hit_path() {
        // All results pre-cached → no solver calls needed.
        let cache = Cache::open(":memory:", false).unwrap();
        let paths = vec!["i1.cnf".to_string(), "i2.cnf".to_string()];
        let id_map = cache.load_instances(&paths).unwrap();

        let config: Config = [("alpha".to_string(), "1.189".to_string())].into();
        let hash = hash_config(&config);
        let id1 = id_map["i1.cnf"];
        let id2 = id_map["i2.cnf"];

        let mut cache = cache;
        cache.put(hash, id1, 0.5, 0.0, "Theorem", 10.0, None).unwrap();
        cache.put(hash, id2, 1.0, 0.0, "Theorem", 10.0, None).unwrap();

        let sched = Scheduler::new(2, "unused".to_string(), 10.0, crate::DebugOptions::default());
        sched
            .submit(
                vec![EvalTask {
                    neighbor_id: 7,
                    config,
                    hash,
                    instances: Arc::new(vec![(id1, paths[0].clone()), (id2, paths[1].clone())]),
                }],
                &cache,
            )
            .unwrap();

        let mut results = vec![];
        for _ in 0..2 {
            results.push(
                sched
                    .results()
                    .recv_timeout(std::time::Duration::from_millis(100))
                    .expect("expected cache hit result"),
            );
        }
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.neighbor_id == 7));
        assert!(results.iter().all(|r| !r.cacheable));
        let runtimes: Vec<f64> = results.iter().map(|r| r.runtime).collect();
        assert!(runtimes.contains(&0.5));
        assert!(runtimes.contains(&1.0));
    }

    #[test]
    fn synthetic_timeout_is_not_cacheable() {
        let mut cache = Cache::open(":memory:", false).unwrap();
        let instance = "i1.cnf".to_string();
        let id_map = cache.load_instances(std::slice::from_ref(&instance)).unwrap();
        let instance_id = id_map[&instance];
        let config: Config = [("alpha".to_string(), "1".to_string())].into();
        let hash = hash_config(&config);
        cache.put(hash, instance_id, 8.0, 0.0, "sat", 10.0, None).unwrap();

        let scheduler = Scheduler::new(1, "unused".to_string(), 5.0, crate::DebugOptions::default());
        scheduler
            .submit(
                vec![EvalTask {
                    neighbor_id: 0,
                    config,
                    hash,
                    instances: Arc::new(vec![(instance_id, instance)]),
                }],
                &cache,
            )
            .unwrap();

        let result = scheduler
            .results()
            .recv_timeout(Duration::from_millis(100))
            .expect("expected synthetic timeout");
        assert_eq!(result.status, "TIMEOUT");
        assert!(!result.cacheable);

        let original = cache.get_batch(hash, &[instance_id], 10.0).unwrap();
        assert_eq!(original[&instance_id].status, "sat");
        assert!((original[&instance_id].runtime - 8.0).abs() < 1e-9);
    }

    #[test]
    fn scheduler_reset_drains_cleanly() {
        let cache = Cache::open(":memory:", false).unwrap();
        let sched = Scheduler::new(2, "unused".to_string(), 10.0, crate::DebugOptions::default());

        // Submit empty batch (nothing to do) then reset immediately.
        sched.submit(vec![], &cache).unwrap();
        sched.reset();
        // Drain — should be empty
        while sched.results().try_recv().is_ok() {}
        // Should be able to submit again without panic
        sched.submit(vec![], &cache).unwrap();
    }

    #[test]
    fn scheduler_reset_terminates_active_solver() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("wrapper.sh");
        let pid_file = dir.path().join("solver.pid");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\necho \"$$\" > '{}'\ntrap 'exit 143' TERM\nsleep 300\n",
                pid_file.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).unwrap();

        let cache = Cache::open(":memory:", false).unwrap();
        let instance = "instance.cnf".to_string();
        let ids = cache.load_instances(std::slice::from_ref(&instance)).unwrap();
        let config: Config = [("alpha".to_string(), "1".to_string())].into();
        let sched = Scheduler::new(1, wrapper.display().to_string(), 300.0, crate::DebugOptions::default());
        sched
            .submit(
                vec![EvalTask {
                    neighbor_id: 0,
                    hash: hash_config(&config),
                    config,
                    instances: Arc::new(vec![(ids[&instance], instance)]),
                }],
                &cache,
            )
            .unwrap();

        let start_deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() {
            assert!(Instant::now() < start_deadline, "solver process did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        let solver_pid: libc::pid_t = fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();

        sched.reset();

        let exit_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let exists = unsafe {
                libc::kill(solver_pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            };
            if !exists {
                break;
            }
            assert!(
                Instant::now() < exit_deadline,
                "solver process {solver_pid} survived scheduler reset"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            sched.results().recv_timeout(Duration::from_millis(200)).is_err(),
            "canceled solver produced a result"
        );
    }

    #[test]
    fn scheduler_queues_batches_not_individual_instances() {
        let cache = Cache::open(":memory:", false).unwrap();
        let paths: Vec<String> = (0..10_000).map(|i| format!("i{i}.cnf")).collect();
        let id_map = cache.load_instances(&paths).unwrap();
        let instances = Arc::new(paths.iter().map(|path| (id_map[path], path.clone())).collect());
        let config: Config = [("alpha".to_string(), "1".to_string())].into();
        let sched = Scheduler::new(4, "true".to_string(), 10.0, crate::DebugOptions::default());

        sched
            .submit(
                vec![EvalTask {
                    neighbor_id: 0,
                    hash: hash_config(&config),
                    config,
                    instances,
                }],
                &cache,
            )
            .unwrap();

        assert_eq!(sched.submitted_work_batches(), 4);
        sched.reset();
    }
}
