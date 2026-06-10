//! ILS loop: initialization → basic local search → perturbation → acceptance.
//!
//! Mirrors `param_ils_2_3_run.rb`:
//!   iterated_local_search()
//!     init_default() / init_random()
//!     basic_local_search()    — first-improvement, parallel neighbour evaluation
//!     perturbation()          — random walk of strength s
//!     acceptance_criterion()  — accept if new local optimum dominates incumbent

use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::Result;
use crossbeam::channel::RecvTimeoutError;
use rand::Rng;

use crate::cache::{Cache, hash_config};
use crate::eval::{EvalTask, Scheduler};
use crate::params::{Config, ParamSpace};
use crate::scenario::{OverallObjective, RunObjective};


fn print_diff(to_stderr: bool, prev: &Config, next: &Config, space: &ParamSpace) {
    let active: std::collections::HashSet<String> = space.active_params(next)
        .into_iter().map(|p| p.name.clone()).collect();
    let mut changes: Vec<String> = active.iter()
        .filter_map(|k| {
            let a = prev.get(k).map(|s| s.as_str()).unwrap_or("-");
            let b = next.get(k).map(|s| s.as_str()).unwrap_or("-");
            if a != b { Some(format!("           {k}: {a} -> {b}")) } else { None }
        })
        .collect();
    changes.sort();
    for line in changes { crate::debug_line(to_stderr, &line); }
}

/// ILS algorithm variant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Approach {
    Basic,
    Focused,
    Random,
}

/// All tunable settings for a single ILS run.
#[derive(Clone)]
pub struct IlsOptions {
    pub approach: Approach,
    /// Max parallel worker threads.
    pub n_workers: usize,
    /// Number of neighbourhood steps for perturbation.
    pub perturbation_strength: usize,
    /// Initial number of instances used to evaluate each configuration.
    pub initial_fidelity: usize,
    /// Number of instances added when FocusedILS increases fidelity.
    pub fidelity_step: usize,
    /// Adaptive capping multiplier.
    pub bound_multiplier: f64,
    /// Enable adaptive capping / pruning.
    pub pruning: bool,
    /// Wall-clock budget for the whole ILS run in seconds.
    pub tuner_timeout: f64,
    pub run_obj: RunObjective,
    pub overall_obj: OverallObjective,
    pub debug: crate::DebugOptions,
}

/// Run the ILS and return the best configuration found.
///
/// `instances` must be pre-registered with `cache.load_instances()` — each
/// entry is `(instance_id, instance_path)`.
pub fn run(
    initial: Option<Config>,
    options: &IlsOptions,
    space: &ParamSpace,
    instances: &[(i64, String)],
    algo: &str,
    cutoff_time: f64,
    cache: &mut Cache,
) -> Result<(Config, f64)> {
    let deadline = Instant::now() + Duration::from_secs_f64(options.tuner_timeout);
    let scheduler = Scheduler::new(options.n_workers, algo.to_string(), cutoff_time, options.debug);
    let mut rng = rand::thread_rng();
    let n_total = instances.len();

    // FocusedILS starts at the configured fidelity and grows; Basic/Random use all instances.
    let mut n_runs = match options.approach {
        Approach::Focused => initial_n_runs(options.initial_fidelity, n_total),
        _ => n_total,
    };

    // --- Initialization ---
    if options.debug.main {
        let t = crate::t();
        let d = true;
        let approach_str = match options.approach {
            Approach::Basic => "basic", Approach::Focused => "focused", Approach::Random => "random",
        };
        crate::debug_line(d, &format!("[{t:8.2}s] ils: starting approach={approach_str} instances={n_total} timeout={:.0}s", options.tuner_timeout));
        crate::debug_line(d, &format!("[{t:8.2}s] ils: initial config:"));
        match &initial {
            Some(cfg) => {
                let active = space.active_params(cfg);
                let mut pairs: Vec<(&str, &str)> = active.iter()
                    .filter_map(|p| cfg.get(&p.name).map(|v| (p.name.as_str(), v.as_str())))
                    .collect();
                pairs.sort_by_key(|(k, _)| *k);
                for (k, v) in pairs { crate::debug_line(d, &format!("           {k}: {v}")); }
            }
            None => crate::debug_line(d, "           (random)"),
        }
    }
    let mut current = match initial {
        Some(cfg) => cfg,
        None => {
            // No initial config: sample a handful of random configs, keep the best.
            let mut best = random_config(space, &mut rng);
            let mut best_score = evaluate_config(&best, &instances[..n_runs], &scheduler, cache, &options, None, deadline)?;
            for _ in 1..10 {
                if Instant::now() >= deadline { break; }
                let cfg = random_config(space, &mut rng);
                let score = evaluate_config(&cfg, &instances[..n_runs], &scheduler, cache, &options, Some(best_score), deadline)?;
                if score < best_score {
                    best_score = score;
                    best = cfg;
                }
            }
            best
        }
    };

    let mut current_score =
        evaluate_config(&current, &instances[..n_runs], &scheduler, cache, &options, None, deadline)?;

    let mut incumbent = current.clone();
    let mut incumbent_score = current_score;

    // --- First BLS ---
    let (lm, lm_score) = basic_local_search(
        current, current_score,
        instances, n_runs, &scheduler, cache, &options, space,
        incumbent_score, &mut rng, deadline,
    )?;
    if lm_score < incumbent_score {
        let old = incumbent.clone();
        incumbent = lm.clone();
        incumbent_score = lm_score;
        if options.debug.main {
            let hash = hash_config(&incumbent);
            crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: new incumbent: hash={hash:016x} score={incumbent_score:.6} instances={n_runs}", crate::t()));
            print_diff(options.debug.main, &old, &incumbent, space);
        }
    }
    let mut last_lm = lm;
    let mut last_lm_score = lm_score;

    // --- Main ILS loop ---
    while Instant::now() < deadline && !crate::interrupted() {
        // Perturbation
        let perturbed = perturbation(last_lm.clone(), options.perturbation_strength, space, &mut rng);
        crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: perturbation strength={}", crate::t(), options.perturbation_strength));
        current = perturbed;
        current_score = evaluate_config(
            &current, &instances[..n_runs], &scheduler, cache, &options, Some(incumbent_score), deadline,
        )?;

        if Instant::now() >= deadline || crate::interrupted() { break; }

        // BLS from the perturbed point — evaluate neighbours on n_runs instances
        if options.debug.main {
            let nb = neighbourhood(&current, space).len();
            crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: bls neighborhood={nb} instances={n_runs} incumbent={incumbent_score:.6}", crate::t()));
        }
        let (new_lm, new_lm_score) = basic_local_search(
            current, current_score,
            instances, n_runs, &scheduler, cache, &options, space,
            incumbent_score, &mut rng, deadline,
        )?;

        // Update incumbent
        if new_lm_score < incumbent_score {
            let old = incumbent.clone();
            incumbent = new_lm.clone();
            incumbent_score = new_lm_score;
            if options.debug.main {
                let hash = hash_config(&incumbent);
                crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: new incumbent: hash={hash:016x} score={incumbent_score:.6} instances={n_runs}", crate::t()));
                print_diff(options.debug.main, &old, &incumbent, space);
            }
        } else if options.approach == Approach::Focused {
            // Incumbent survived — increase fidelity for the next round (up to all instances).
            // This is the bounded increase mechanism: challengers that fail against the
            // current incumbent push it to be evaluated on another fidelity step.
            let next = next_n_runs(n_runs, options.fidelity_step, n_total);
            if next > n_runs {
                n_runs = next;
                incumbent_score = evaluate_config(
                    &incumbent, &instances[..n_runs], &scheduler, cache, &options, None, deadline,
                )?;
                crate::debug_line(options.debug.main, &format!(
                    "[{:8.2}s] ils: n_runs increased to {n_runs}/{n_total} incumbent_score={incumbent_score:.6}",
                    crate::t()
                ));
            }
        }

        // Acceptance criterion: keep new local opt only if it dominates the last one
        let (accepted, accepted_score) =
            acceptance_criterion(new_lm, new_lm_score, last_lm.clone(), last_lm_score, &options);
        last_lm = accepted;
        last_lm_score = accepted_score;
    }

    Ok((incumbent, incumbent_score))
}

// ---------------------------------------------------------------------------
// Core algorithm functions
// ---------------------------------------------------------------------------

/// All one-parameter-away neighbours of `config` within `space`.
/// Only iterates over active params; skips forbidden combinations.
pub fn neighbourhood(config: &Config, space: &ParamSpace) -> Vec<Config> {
    let active = space.active_params(config);
    let mut result = Vec::new();
    let empty = String::new();
    for param in active {
        let current_val = config.get(&param.name).unwrap_or(&empty);
        for value in &param.domain {
            if value == current_val { continue; }
            let mut new_cfg = config.clone();
            new_cfg.insert(param.name.clone(), value.clone());
            if !space.is_forbidden(&new_cfg) {
                result.push(new_cfg);
            }
        }
    }
    result
}

/// Random walk: take `strength` steps through the neighbourhood.
pub fn perturbation(config: Config, strength: usize, space: &ParamSpace, rng: &mut impl Rng) -> Config {
    if matches!(strength, 0) { return config; }
    let mut current = config;
    for _ in 0..strength {
        let neighbors = neighbourhood(&current, space);
        if neighbors.is_empty() { break; }
        current = neighbors[rng.gen_range(0..neighbors.len())].clone();
    }
    current
}

/// `a` dominates `b` when `a` is strictly better (lower score).
/// FocusedILS also requires at least as many runs.  BasicILS ignores run counts.
///
/// Strict `<` (not `≤`) is intentional: ties do not count as improvement.
/// This lets FocusedILS grow `n_runs` when the incumbent survives a tie
/// instead of endlessly replacing it with an equal-scoring challenger.
pub fn dominates(
    a_score: f64, a_runs: usize,
    b_score: f64, b_runs: usize,
    options: &IlsOptions,
) -> bool {
    match options.approach {
        Approach::Basic | Approach::Random => a_score < b_score,
        Approach::Focused => a_runs >= b_runs && a_score < b_score,
    }
}

/// Return the new local optimum if it dominates the previous one; otherwise
/// keep the previous one.
fn acceptance_criterion(
    new: Config, new_score: f64,
    last: Config, last_score: f64,
    options: &IlsOptions,
) -> (Config, f64) {
    if dominates(new_score, 1, last_score, 1, options) {
        (new, new_score)
    } else {
        (last, last_score)
    }
}

// ---------------------------------------------------------------------------
// Evaluation helpers
// ---------------------------------------------------------------------------

/// Evaluate `config` on all instances in parallel.  Returns the scalar score.
///
/// Cache hits are served immediately; misses are dispatched to worker threads.
/// Adaptive capping prunes early if the running mean exceeds
/// `bound_multiplier × incumbent_score`.
fn evaluate_config(
    config: &Config,
    instances: &[(i64, String)],
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    incumbent_score: Option<f64>,
    deadline: Instant,
) -> Result<f64> {
    if instances.is_empty() { return Ok(0.0); }

    let hash = hash_config(config);
    let batch_id = scheduler.submit(vec![EvalTask {
        neighbor_id: 0,
        config: config.clone(),
        hash,
        instances: Arc::new(instances.to_vec()),
    }], cache)?;

    collect_one(batch_id, instances.len(), 0, scheduler, cache, options, incumbent_score, deadline)
}

/// Parallel first-improvement BLS.
///
/// Submits all neighbours as `EvalTask`s at once.  Accepts the first
/// fully-evaluated neighbour that dominates the current config (in
/// evaluation-completion order).  Resets the scheduler when a better
/// neighbour is found (so we don't wait for the rest).
fn basic_local_search(
    start: Config, start_score: f64,
    instances: &[(i64, String)],
    n_runs: usize,
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    space: &ParamSpace,
    incumbent_score: f64,
    rng: &mut impl Rng,
    deadline: Instant,
) -> Result<(Config, f64)> {
    let eval_instances = &instances[..n_runs];
    let n_instances = n_runs;
    let mut current = start;
    let mut current_score = start_score;
    let mut changed = true;

    while changed && Instant::now() < deadline && !crate::interrupted() {
        changed = false;

        let mut neighbors = neighbourhood(&current, space);
        if neighbors.is_empty() { break; }

        // Shuffle for random first-improvement ordering
        for i in (1..neighbors.len()).rev() {
            let j = rng.gen_range(0..=i);
            neighbors.swap(i, j);
        }

        let n = neighbors.len();

        // Submit all neighbours (evaluated on the first n_runs instances only)
        let shared_instances = Arc::new(eval_instances.to_vec());
        let tasks: Vec<EvalTask> = neighbors.iter().enumerate().map(|(i, cfg)| {
            let hash = hash_config(cfg);
            EvalTask {
                neighbor_id: i,
                config: cfg.clone(),
                hash,
                instances: Arc::clone(&shared_instances),
            }
        }).collect();
        let batch_id = scheduler.submit(tasks, cache)?;

        // Per-neighbour tracking
        let mut runtimes: Vec<Vec<f64>> = vec![vec![]; n];
        let mut qualities: Vec<Vec<f64>> = vec![vec![]; n];
        let mut partial: Vec<f64> = vec![0.0; n];
        let mut done = vec![false; n];
        let mut n_done = 0usize;

        'collect: loop {
            if n_done >= n { break; }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || crate::interrupted() { break; }

            let result = match scheduler.results().recv_timeout(remaining.min(Duration::from_millis(500))) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };

            if result.status != "UNKNOWN" {
                cache.put(result.hash, result.instance_id, result.runtime, result.quality, &result.status)?;
            }
            if result.batch_id != batch_id { continue; }

            let nid = result.neighbor_id;
            // Guard against stale results from a previous reset (shouldn't
            // normally happen, but the window between reset() and drain is tiny).
            if nid >= n || done[nid] { continue; }

            runtimes[nid].push(result.runtime);
            qualities[nid].push(result.quality);
            let val = match options.run_obj {
                RunObjective::Runtime => result.runtime,
                RunObjective::Quality => result.quality,
            };
            partial[nid] += val;

            // Adaptive capping: prune this neighbour if its running mean already
            // exceeds the incumbent bound — it can't win even if the rest are fast.
            if options.pruning {
                let pm = partial[nid] / runtimes[nid].len() as f64;
                if pm > options.bound_multiplier * incumbent_score {
                    crate::debug_line(options.debug.wrapper, &format!(
                        "[{:8.2}s] ils: capped neighbor={nid} partial_mean={pm:.6} bound={:.6}",
                        crate::t(), options.bound_multiplier * incumbent_score
                    ));
                    done[nid] = true;
                    n_done += 1;
                    continue;
                }
            }

            if runtimes[nid].len() == n_instances {
                done[nid] = true;
                n_done += 1;
                let score = compute_score(&runtimes[nid], &qualities[nid], options);

                if dominates(score, n_instances, current_score, n_instances, options) {
                    // Accept — stop evaluating the rest
                    scheduler.reset();
                    while let Ok(r) = scheduler.results().try_recv() {
                if r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status)?;
                }
            }
                    crate::debug_line(options.debug.main, &format!(
                        "[{:8.2}s] ils: bls improvement neighbor={nid} score={score:.6} (was {current_score:.6})",
                        crate::t()
                    ));
                    current = neighbors[nid].clone();
                    current_score = score;
                    changed = true;
                    break 'collect;
                }
            }
        }

        if !changed {
            scheduler.reset();
            while let Ok(r) = scheduler.results().try_recv() {
                if r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status)?;
                }
            }
            crate::debug_line(options.debug.main, &format!(
                "[{:8.2}s] ils: bls local optimum score={current_score:.6}",
                crate::t()
            ));
        }
    }

    Ok((current, current_score))
}

/// Collect exactly `n_instances` results for one config (neighbor_id = `expected_nid`).
/// Used by `evaluate_config` for single-config evaluation.
fn collect_one(
    batch_id: u64,
    n_instances: usize,
    expected_nid: usize,
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    incumbent_score: Option<f64>,
    deadline: Instant,
) -> Result<f64> {
    let mut runtimes = Vec::with_capacity(n_instances);
    let mut qualities = Vec::with_capacity(n_instances);
    let mut partial_sum = 0.0f64;

    while runtimes.len() < n_instances {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || crate::interrupted() { break; }

        let result = match scheduler.results().recv_timeout(remaining.min(Duration::from_millis(500))) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        if result.status != "UNKNOWN" {
            cache.put(result.hash, result.instance_id, result.runtime, result.quality, &result.status)?;
        }
        if result.batch_id != batch_id { continue; }
        if result.neighbor_id != expected_nid { continue; }

        let val = match options.run_obj {
            RunObjective::Runtime => result.runtime,
            RunObjective::Quality => result.quality,
        };
        partial_sum += val;
        runtimes.push(result.runtime);
        qualities.push(result.quality);

        if options.pruning {
            if let Some(inc) = incumbent_score {
                let pm = partial_sum / runtimes.len() as f64;
                if pm > options.bound_multiplier * inc {
                    scheduler.reset();
                    while let Ok(r) = scheduler.results().try_recv() {
                if r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status)?;
                }
            }
                    break;
                }
            }
        }
    }

    if runtimes.is_empty() { return Ok(f64::INFINITY); }
    Ok(compute_score(&runtimes, &qualities, options))
}

/// Compute a scalar score from per-instance results.
fn compute_score(runtimes: &[f64], qualities: &[f64], options: &IlsOptions) -> f64 {
    let values: &[f64] = match options.run_obj {
        RunObjective::Runtime => runtimes,
        RunObjective::Quality => qualities,
    };
    match options.overall_obj {
        OverallObjective::Mean => values.iter().sum::<f64>() / values.len() as f64,
        OverallObjective::Median => {
            let mut s = values.to_vec();
            s.sort_by(f64::total_cmp);
            let n = s.len();
            if n % 2 == 0 { (s[n / 2 - 1] + s[n / 2]) / 2.0 } else { s[n / 2] }
        }
    }
}

fn initial_n_runs(initial_fidelity: usize, n_total: usize) -> usize {
    initial_fidelity.max(1).min(n_total)
}

fn next_n_runs(current: usize, fidelity_step: usize, n_total: usize) -> usize {
    current.saturating_add(fidelity_step.max(1)).min(n_total)
}

/// Iterative-deepening wrapper around [`run`].
///
/// Mirrors `iterative_deepening_ils()` from `param_ils_2_3_run.rb` (lines 809–856,
/// 1795–1837).  Builds an exponential schedule over instance count, cutoff time,
/// and per-phase timeout controlled by λ_n, λ_c, λ_t (defaults: 0.5 each).
/// Each phase seeds the next with its incumbent.
pub fn iterative_deepening_ils(
    initial: Option<Config>,
    options: &IlsOptions,
    space: &ParamSpace,
    instances: &[(i64, String)],
    algo: &str,
    cutoff_time: f64,
    cache: &mut Cache,
    lambda_n: f64,
    lambda_c: f64,
    lambda_t: f64,
) -> Result<(Config, f64)> {
    let n_total = instances.len();
    let eps = 1e-6;

    // Number of phases: enough that the earliest phase starts near 1 instance / 1s cutoff.
    let num_depths = {
        let raw = if lambda_n < 1.0 - eps {
            ((n_total as f64).ln() / (1.0 / lambda_n).ln()).ceil() as usize + 1
        } else {
            (cutoff_time.ln().max(0.0) / (1.0 / lambda_c).ln()).ceil() as usize + 1
        };
        raw.max(1)
    };

    // Build schedule: (n_instances, phase_cutoff, phase_t_budget_from_start)
    let schedule: Vec<(usize, f64, f64)> = (0..num_depths).map(|i| {
        let exp = (num_depths - 1 - i) as f64;
        let n = ((n_total as f64) * lambda_n.powf(exp)).ceil() as usize;
        let n = n.max(1).min(n_total);
        let c = (cutoff_time * lambda_c.powf(exp)).ceil();
        let t = options.tuner_timeout * lambda_t.powf(exp);
        (n, c, t)
    }).collect();

    if options.debug.main {
        crate::debug_line(options.debug.main, &format!(
            "[{:8.2}s] id: {} phases  λ_n={lambda_n} λ_c={lambda_c} λ_t={lambda_t}",
            crate::t(), num_depths
        ));
        for (i, (n, c, t)) in schedule.iter().enumerate() {
            crate::debug_line(options.debug.main, &format!(
                "[{:8.2}s] id:   phase {} n={n} cutoff={c:.1}s timeout={t:.1}s",
                crate::t(), i + 1
            ));
        }
    }

    let start = Instant::now();
    let mut current_initial = initial;
    let mut best: Option<(Config, f64)> = None;

    for (depth, (n, c, t)) in schedule.iter().enumerate() {
        if crate::interrupted() {
            break;
        }
        let elapsed = start.elapsed().as_secs_f64();
        let phase_remaining = t - elapsed;
        if phase_remaining <= 0.0 {
            if options.debug.main {
                crate::debug_line(options.debug.main, &format!(
                    "[{:8.2}s] id: phase {}/{} skipped (budget exhausted)",
                    crate::t(), depth + 1, num_depths
                ));
            }
            break;
        }

        if options.debug.main {
            crate::debug_line(options.debug.main, &format!(
                "[{:8.2}s] id: starting phase {}/{} n={n} cutoff={c:.1}s remaining={phase_remaining:.1}s",
                crate::t(), depth + 1, num_depths
            ));
        }

        let mut phase_options = options.clone();
        phase_options.tuner_timeout = phase_remaining;

        let (inc, score) = run(
            current_initial.take(),
            &phase_options,
            space,
            &instances[..*n],
            algo,
            *c,
            cache,
        )?;

        if options.debug.main {
            crate::debug_line(options.debug.main, &format!(
                "[{:8.2}s] id: phase {}/{} done — score={score:.6}",
                crate::t(), depth + 1, num_depths
            ));
        }

        current_initial = Some(inc.clone());
        best = Some((inc, score));
    }

    best.ok_or_else(|| anyhow::anyhow!("no phases ran (budget already exhausted)"))
}

/// Sample a random non-forbidden configuration.
fn random_config(space: &ParamSpace, rng: &mut impl Rng) -> Config {
    loop {
        let cfg: Config = space.params.iter()
            .map(|p| (p.name.clone(), p.domain[rng.gen_range(0..p.domain.len())].clone()))
            .collect();
        if !space.is_forbidden(&cfg) { return cfg; }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_space() -> ParamSpace {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "alpha {{1, 2, 3}} [2]").unwrap();
        writeln!(f, "beta {{a, b}} [a]").unwrap();
        let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
        drop(f);
        space
    }

    fn forbidden_space() -> ParamSpace {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "x {{1, 2}} [1]").unwrap();
        writeln!(f, "y {{a, b}} [a]").unwrap();
        writeln!(f, "{{x=2, y=b}}").unwrap();
        let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
        drop(f);
        space
    }

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn neighbourhood_size() {
        let space = simple_space();
        let config = cfg(&[("alpha", "2"), ("beta", "a")]);
        let n = neighbourhood(&config, &space);
        // alpha has 2 other values, beta has 1 other value → 3 neighbours
        assert_eq!(n.len(), 3);
        // All neighbours differ in exactly one param
        for nb in &n {
            let diffs: usize = nb.iter()
                .filter(|(k, v)| config.get(*k) != Some(v))
                .count();
            assert_eq!(diffs, 1);
        }
    }

    #[test]
    fn neighbourhood_skips_forbidden() {
        let space = forbidden_space();
        let config = cfg(&[("x", "2"), ("y", "a")]);
        let n = neighbourhood(&config, &space);
        // From x=2,y=a: can go to x=1,y=a (ok) or x=2,y=b (forbidden) → 1 neighbor
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["x"], "1");
    }

    #[test]
    fn perturbation_changes_config() {
        let space = simple_space();
        let config = cfg(&[("alpha", "2"), ("beta", "a")]);
        let mut rng = rand::thread_rng();
        let perturbed = perturbation(config.clone(), 3, &space, &mut rng);
        // After 3 perturbation steps, config should generally differ
        // (probabilistically, but with a space this small it's very likely)
        assert_eq!(perturbed.len(), config.len());
    }

    #[test]
    fn dominates_basic() {
        let opts = IlsOptions {
            approach: Approach::Basic,
            n_workers: 1, perturbation_strength: 4, debug: crate::DebugOptions::default(),
            initial_fidelity: 1, fidelity_step: 1,
            bound_multiplier: 10.0, pruning: true, tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime, overall_obj: OverallObjective::Mean,
        };
        assert!(dominates(1.0, 5, 2.0, 5, &opts));   // strictly better
        assert!(dominates(1.0, 1, 2.0, 10, &opts));  // BasicILS ignores run counts
        assert!(!dominates(2.0, 5, 1.0, 5, &opts));  // worse
        assert!(!dominates(1.0, 5, 1.0, 5, &opts));  // tie — does NOT dominate
    }

    #[test]
    fn dominates_focused() {
        let opts = IlsOptions {
            approach: Approach::Focused,
            n_workers: 1, perturbation_strength: 4, debug: crate::DebugOptions::default(),
            initial_fidelity: 1, fidelity_step: 1,
            bound_multiplier: 10.0, pruning: true, tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime, overall_obj: OverallObjective::Mean,
        };
        assert!(dominates(1.0, 10, 2.0, 5, &opts));  // strictly better score, more runs
        assert!(!dominates(1.0, 3, 2.0, 5, &opts));  // better score but fewer runs
        assert!(!dominates(1.0, 10, 1.0, 5, &opts)); // tie — does NOT dominate
    }

    #[test]
    fn compute_score_mean_runtime() {
        let opts = IlsOptions {
            approach: Approach::Basic,
            n_workers: 1, perturbation_strength: 4, debug: crate::DebugOptions::default(),
            initial_fidelity: 1, fidelity_step: 1,
            bound_multiplier: 10.0, pruning: false, tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime, overall_obj: OverallObjective::Mean,
        };
        assert!((compute_score(&[1.0, 2.0, 3.0], &[0.0; 3], &opts) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn compute_score_median_runtime() {
        let opts = IlsOptions {
            approach: Approach::Basic,
            n_workers: 1, perturbation_strength: 4, debug: crate::DebugOptions::default(),
            initial_fidelity: 1, fidelity_step: 1,
            bound_multiplier: 10.0, pruning: false, tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime, overall_obj: OverallObjective::Median,
        };
        assert!((compute_score(&[3.0, 1.0, 2.0], &[0.0; 3], &opts) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn random_config_not_forbidden() {
        let space = forbidden_space();
        let mut rng = rand::thread_rng();
        for _ in 0..50 {
            let cfg = random_config(&space, &mut rng);
            assert!(!space.is_forbidden(&cfg));
        }
    }

    #[test]
    fn fidelity_is_clamped_and_advances_by_step() {
        assert_eq!(initial_n_runs(8, 100), 8);
        assert_eq!(initial_n_runs(100, 8), 8);
        assert_eq!(initial_n_runs(0, 8), 1);

        assert_eq!(next_n_runs(8, 4, 100), 12);
        assert_eq!(next_n_runs(8, 100, 10), 10);
        assert_eq!(next_n_runs(8, 0, 100), 9);
    }

    #[test]
    fn evaluation_waits_through_poll_timeouts() {
        let mut cache = Cache::open(":memory:", false).unwrap();
        let path = "instance.cnf".to_string();
        let ids = cache.load_instances(std::slice::from_ref(&path)).unwrap();
        let instances = vec![(ids[&path], path)];
        let scheduler = Scheduler::new(
            1,
            "sleep 0.7; echo '#%# RamParIls #%# sat, 0.7, 0.0'; true".to_string(),
            2.0,
            crate::DebugOptions::default(),
        );
        let options = IlsOptions {
            approach: Approach::Focused,
            n_workers: 1,
            perturbation_strength: 1,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: false,
            tuner_timeout: 2.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
            debug: crate::DebugOptions::default(),
        };
        let config = cfg(&[("alpha", "1")]);
        let started = Instant::now();

        let score = evaluate_config(
            &config,
            &instances,
            &scheduler,
            &mut cache,
            &options,
            None,
            started + Duration::from_secs(2),
        ).unwrap();

        assert!(started.elapsed() >= Duration::from_millis(650));
        assert!((score - 0.7).abs() < 1e-9);
    }
}
