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
//!  │                          misses ────────────────────────▶ work_rx
//!  │                                │                          │── run_solver ──▶
//!  │◀── results().recv() ──────────────────────────────────────│
//!  │                                │                          │
//!  │── reset() ────────────────────▶│  (stop flag set)         │
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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam::channel::{unbounded, Receiver, Sender};
use anyhow::Result;

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
    pub instances: Vec<(i64, String)>,
}

/// Result of one `(neighbor, instance)` evaluation.
pub struct TaskResult {
    pub neighbor_id: usize,
    pub instance_id: i64,
    /// Hash of the config (for cache write-back).
    pub hash: u64,
    pub runtime: f64,
    pub quality: f64,
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
        if n % 2 == 0 { (s[n / 2 - 1] + s[n / 2]) / 2.0 } else { s[n / 2] }
    }
}

// ---------------------------------------------------------------------------
// Internal types (worker-side)
// ---------------------------------------------------------------------------

struct WorkItem {
    neighbor_id: usize,
    config: Config,
    hash: u64,
    instance_id: i64,
    instance_path: String,
    cutoff_time: f64,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

pub struct Scheduler {
    work_tx: Sender<WorkItem>,
    result_tx: Sender<TaskResult>, // used in submit() for cache hits
    result_rx: Receiver<TaskResult>,
    stop: Arc<AtomicBool>,
    cutoff_time: f64,
    debug: bool,
    _workers: Vec<std::thread::JoinHandle<()>>,
}

impl Scheduler {
    /// Spawn `n_workers` worker threads.  Each loops waiting for `WorkItem`s,
    /// runs the solver, and sends `TaskResult`s back.
    pub fn new(n_workers: usize, algo: String, cutoff_time: f64, debug: bool, debug_wrapper: bool, debug_solver: bool) -> Self {
        let (work_tx, work_rx) = unbounded::<WorkItem>();
        let (result_tx, result_rx) = unbounded::<TaskResult>();
        let stop = Arc::new(AtomicBool::new(false));

        let workers = (0..n_workers)
            .map(|_| {
                let work_rx: Receiver<WorkItem> = work_rx.clone();
                let result_tx: Sender<TaskResult> = result_tx.clone();
                let stop = Arc::clone(&stop);
                let algo = algo.clone();

                std::thread::spawn(move || {
                    while let Ok(item) = work_rx.recv() {
                        if stop.load(Ordering::Relaxed) {
                            // Drain without processing when stopped.
                            continue;
                        }
                        let (runtime, quality) =
                            run_solver_inner(&algo, &item.config, item.hash, &item.instance_path, item.cutoff_time, debug_wrapper, debug_solver);
                        // Still send the result even if stop was set mid-call;
                        // ILS drains these as part of the reset() protocol.
                        let _ = result_tx.send(TaskResult {
                            neighbor_id: item.neighbor_id,
                            instance_id: item.instance_id,
                            hash: item.hash,
                            runtime,
                            quality,
                        });
                    }
                })
            })
            .collect();

        crate::debug_line(debug, &format!("[{:8.2}s] eval: scheduler started workers={n_workers}", crate::t()));
        Scheduler { work_tx, result_tx, result_rx, stop, cutoff_time, debug, _workers: workers }
    }

    /// Dispatch one batch of tasks.
    ///
    /// For each task, issues one bulk cache query; hits go directly to the
    /// result channel, misses become `WorkItem`s sent to workers.
    ///
    /// Clears the stop flag so workers start processing again.
    pub fn submit(&self, tasks: Vec<EvalTask>, cache: &Cache) -> Result<()> {
        self.stop.store(false, Ordering::Relaxed);

        let (mut n_hits, mut n_misses) = (0usize, 0usize);
        for task in tasks {
            let ids: Vec<i64> = task.instances.iter().map(|(id, _)| *id).collect();
            let hits: HashMap<i64, CachedResult> = cache.get_batch(task.hash, &ids)?;

            for (instance_id, instance_path) in task.instances {
                if let Some(cached) = hits.get(&instance_id) {
                    n_hits += 1;
                    let _ = self.result_tx.send(TaskResult {
                        neighbor_id: task.neighbor_id,
                        instance_id,
                        hash: task.hash,
                        runtime: cached.runtime,
                        quality: cached.quality,
                    });
                } else {
                    n_misses += 1;
                    let _ = self.work_tx.send(WorkItem {
                        neighbor_id: task.neighbor_id,
                        config: task.config.clone(),
                        hash: task.hash,
                        instance_id,
                        instance_path,
                        cutoff_time: self.cutoff_time,
                    });
                }
            }
        }
        crate::debug_line(self.debug, &format!("[{:8.2}s] eval: submitted tasks={} hits={n_hits} misses={n_misses}", crate::t(), n_hits + n_misses));
        Ok(())
    }

    /// The result channel — ILS reads `TaskResult`s from here as they arrive.
    pub fn results(&self) -> &Receiver<TaskResult> {
        &self.result_rx
    }

    /// Signal workers to stop processing new items.
    ///
    /// Workers finish their current solver call then skip remaining queued
    /// items.  The ILS should drain `results()` after calling this to consume
    /// any in-flight results before the next `submit()`.
    pub fn reset(&self) {
        self.stop.store(true, Ordering::Relaxed);
        crate::debug_line(self.debug, &format!("[{:8.2}s] eval: reset", crate::t()));
    }
}

// ---------------------------------------------------------------------------
// Solver subprocess
// ---------------------------------------------------------------------------

/// Invoke the solver and return `(runtime, quality)`.
///
/// Command format:
/// ```text
/// algo  instance  cutoff_time  -key val ...
/// ```
/// The solver must print a result line to stdout:
/// ```text
/// #%# RamParIls #%# <OK|TIMEOUT|CRASHED>, <runtime>, <quality>
/// ```
///
/// On crash or missing result line, returns `(cutoff_time, 0.0)`.
///
/// # Security note
/// The algo string is passed to `sh -c` for shell compatibility.
/// Only trusted scenario files should be used.
pub(crate) fn run_solver_inner(
    algo: &str,
    config: &Config,
    hash: u64,
    instance: &str,
    cutoff_time: f64,
    debug_wrapper: bool,
    debug_solver: bool,
) -> (f64, f64) {
    let mut pairs: Vec<(&String, &String)> = config.iter().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);
    let paramstring = pairs
        .iter()
        .map(|(k, v)| format!("-{k} {v}"))
        .collect::<Vec<_>>()
        .join(" ");

    let cmd = format!("{algo} {instance} {cutoff_time} {paramstring}");
    crate::debug_line(debug_wrapper, &format!("[{:8.2}s] wrapper: {cmd}", crate::t()));

    let output = std::process::Command::new("sh")
        .args(["-c", &cmd])
        .output();

    let (runtime, quality) = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_solver_output(&stdout, cutoff_time)
        }
        Err(_) => (cutoff_time, 0.0),
    };
    crate::debug_line(debug_solver, &format!("[{:8.2}s] solver: hash={hash:016x} instance={instance} runtime={runtime:.6} quality={quality:.6}", crate::t()));
    (runtime, quality)
}

fn parse_solver_output(output: &str, cutoff_time: f64) -> (f64, f64) {
    for line in output.lines() {
        let rest = match line.strip_prefix("#%# RamParIls #%# ") {
            Some(r) => r,
            None => continue,
        };

        // Format: status, runtime, quality
        let mut parts = rest.splitn(3, ',');
        let _status = parts.next();
        let runtime = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
        let quality = parts.next().and_then(|s| s.trim().parse::<f64>().ok());

        if let (Some(rt), Some(q)) = (runtime, quality) {
            return (rt.min(cutoff_time), q);
        }
    }
    (cutoff_time, 0.0) // no result line — treat as timeout/crash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Cache, hash_config};

    #[test]
    fn parse_ok_line() {
        let (rt, q) = parse_solver_output(
            "some preamble\n#%# RamParIls #%# OK, 1.23, 42.0\n",
            10.0,
        );
        assert!((rt - 1.23).abs() < 1e-9);
        assert!((q - 42.0).abs() < 1e-9);
    }

    #[test]
    fn parse_timeout_line() {
        let (rt, _) = parse_solver_output("#%# RamParIls #%# TIMEOUT, 5.0, 0.0", 10.0);
        assert!((rt - 5.0).abs() < 1e-9);
    }

    #[test]
    fn parse_caps_at_cutoff() {
        let (rt, _) = parse_solver_output("#%# RamParIls #%# OK, 99.9, 0.0", 10.0);
        assert!((rt - 10.0).abs() < 1e-9);
    }

    #[test]
    fn parse_missing_returns_cutoff() {
        let (rt, q) = parse_solver_output("no result here", 5.0);
        assert!((rt - 5.0).abs() < 1e-9);
        assert!((q - 0.0).abs() < 1e-9);
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

        cache.put(hash, id1, 0.5, 0.0).unwrap();
        cache.put(hash, id2, 1.0, 0.0).unwrap();

        let sched = Scheduler::new(2, "unused".to_string(), 10.0, false, false, false);
        sched.submit(vec![EvalTask {
            neighbor_id: 7,
            config,
            hash,
            instances: vec![(id1, paths[0].clone()), (id2, paths[1].clone())],
        }], &cache).unwrap();

        let mut results = vec![];
        for _ in 0..2 {
            results.push(sched.results().recv_timeout(
                std::time::Duration::from_millis(100)
            ).expect("expected cache hit result"));
        }
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.neighbor_id == 7));
        let runtimes: Vec<f64> = results.iter().map(|r| r.runtime).collect();
        assert!(runtimes.contains(&0.5));
        assert!(runtimes.contains(&1.0));
    }

    #[test]
    fn scheduler_reset_drains_cleanly() {
        let cache = Cache::open(":memory:", false).unwrap();
        let sched = Scheduler::new(2, "unused".to_string(), 10.0, false, false, false);

        // Submit empty batch (nothing to do) then reset immediately.
        sched.submit(vec![], &cache).unwrap();
        sched.reset();
        // Drain — should be empty
        while sched.results().try_recv().is_ok() {}
        // Should be able to submit again without panic
        sched.submit(vec![], &cache).unwrap();
    }
}
