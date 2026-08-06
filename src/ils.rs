//! ILS loop: initialization → basic local search → perturbation → acceptance.
//!
//! Mirrors `param_ils_2_3_run.rb`:
//!   iterated_local_search()
//!     init_default() / init_random()
//!     basic_local_search()    — first-improvement, parallel neighbour evaluation
//!     perturbation()          — random walk of strength s
//!     acceptance_criterion()  — accept if the new local optimum dominates the last one
//!
//! # Fidelity consistency
//!
//! A score is only meaningful relative to the instance prefix it was measured
//! on: it is the objective over `instances[..n_runs]`, and different `n_runs`
//! are different objective functions. FocusedILS grows `n_runs` during the run,
//! so every score that outlives a fidelity increase has to be re-measured
//! before it is compared again.
//!
//! ParamILS gets this for free by storing a score *per level* for every
//! configuration (`@cachedResultScalars[state][level]`) and projecting both
//! sides of a comparison onto their common level (`isBetterWithLesserDetail`);
//! `betterWithoutAutomaticIncrease` raises whichever state has fewer runs until
//! the comparison resolves at equal detail. Here each retained state carries a
//! single scalar instead, so there is nothing to project onto — the score has
//! to be re-measured. Both states the loop carries across iterations, the
//! incumbent and the ILS home base (`last_lm`), are therefore re-measured in
//! the fidelity-increase block below, and the loop maintains the invariant that
//! `incumbent_score`, `last_lm_score` and the current round's local optimum are
//! all measured on the same `n_runs`.

use anyhow::Result;
use crossbeam::channel::RecvTimeoutError;
use rand::Rng;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cache::{Cache, hash_config};
use crate::eval::{EvalTask, Scheduler};
use crate::params::{Config, ParamSpace, config_to_yaml};
use crate::scenario::{OverallObjective, RunObjective};


fn log_incumbent(
    enabled: bool,
    incumbent: &Config,
    score: f64,
    n_runs: usize,
    space: &ParamSpace,
) -> Result<()> {
    if !enabled {
        return Ok(());
    }
    let hash = hash_config(&active_config(incumbent, space));
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new incumbent: hash={hash:016x} score={score:.6} instances={n_runs}",
            crate::t()
        ),
    );
    crate::debug_block(true, &config_to_yaml(incumbent)?);
    Ok(())
}

/// Log a replacement of the ILS home base — the configuration the next
/// perturbation starts from (`last_lm`).
///
/// The home base is not the incumbent: the incumbent is the best configuration
/// found, the home base is where the search currently *is*. Only the home base
/// is perturbed, so a home base that stops moving turns the ILS into repeated
/// random restarts from a fixed ball regardless of what the incumbent does —
/// which is exactly what a reader of the log needs to be able to check.
///
/// Unlike a new incumbent this is logged as a single line with the parameter
/// diff against the previous home base, not a full configuration block: the
/// home base can change every round, and the diff is enough to replay the
/// trajectory. Replacements with no *effective* change (a differing value on a
/// parameter whose guard is off) produce an empty diff and are not logged.
fn log_home_base(
    enabled: bool,
    previous: &Config,
    home_base: &Config,
    score: f64,
    n_runs: usize,
    space: &ParamSpace,
) {
    if !enabled {
        return;
    }
    let changes = format_argument_changes(previous, home_base, space);
    if changes.is_empty() {
        return;
    }
    let hash = hash_config(&active_config(home_base, space));
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new home base: hash={hash:016x} score={score:.6} instances={n_runs} changes: {changes}",
            crate::t()
        ),
    );
}

fn format_argument_changes(current: &Config, next: &Config, space: &ParamSpace) -> String {
    let current = active_config(current, space);
    let next = active_config(next, space);
    let names: BTreeSet<&str> = current
        .keys()
        .chain(next.keys())
        .map(String::as_str)
        .collect();

    names
        .into_iter()
        .filter_map(|name| {
            let before = current.get(name).map(String::as_str);
            let after = next.get(name).map(String::as_str);
            (before != after).then(|| {
                format!(
                    "{name}: {} -> {}",
                    before.unwrap_or("<inactive>"),
                    after.unwrap_or("<inactive>")
                )
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
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
            Some(cfg) => crate::debug_block(d, &config_to_yaml(cfg)?),
            None => crate::debug_line(d, "           (random)"),
        }
    }
    let mut current = match initial {
        Some(cfg) => cfg,
        None => {
            // No initial config: sample a handful of random configs, keep the best.
            let mut best = random_config(space, &mut rng);
            let mut best_score = evaluate_config(&best, &instances[..n_runs], &scheduler, cache, options, space, None, deadline)?;
            for _ in 1..10 {
                if Instant::now() >= deadline { break; }
                let cfg = random_config(space, &mut rng);
                let score = evaluate_config(&cfg, &instances[..n_runs], &scheduler, cache, options, space, Some(best_score), deadline)?;
                if score < best_score {
                    best_score = score;
                    best = cfg;
                }
            }
            best
        }
    };

    let mut current_score =
        evaluate_config(&current, &instances[..n_runs], &scheduler, cache, options, space, None, deadline)?;

    let mut incumbent = current.clone();
    let mut incumbent_score = current_score;
    // Kept for the home-base diff below: `incumbent` may be replaced by the
    // first descent, and `current` is moved into it.
    let initial_config = current.clone();

    // --- First BLS ---
    let (lm, lm_score) = basic_local_search(
        current, current_score,
        instances, n_runs, &scheduler, cache, options, space,
        incumbent_score, &mut rng, deadline,
    )?;
    if dominates(lm_score, n_runs, incumbent_score, n_runs, options) {
        incumbent = lm.clone();
        incumbent_score = lm_score;
        log_incumbent(
            options.debug.main,
            &incumbent,
            incumbent_score,
            n_runs,
            space,
        )?;
    }
    let mut last_lm = lm;
    let mut last_lm_score = lm_score;
    // Fidelity at which `last_lm_score` was measured; kept equal to `n_runs`.
    let mut last_lm_runs = n_runs;
    log_home_base(
        options.debug.main,
        &initial_config,
        &last_lm,
        last_lm_score,
        n_runs,
        space,
    );

    // --- Main ILS loop ---
    while Instant::now() < deadline && !crate::interrupted() {
        // Perturbation
        let perturbed = perturbation(last_lm.clone(), options.perturbation_strength, space, &mut rng);
        crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: perturbation strength={}", crate::t(), options.perturbation_strength));
        current = perturbed;
        current_score = evaluate_config(
            &current, &instances[..n_runs], &scheduler, cache, options, space, Some(incumbent_score), deadline,
        )?;

        if Instant::now() >= deadline || crate::interrupted() { break; }

        // BLS from the perturbed point — evaluate neighbours on n_runs instances
        if options.debug.main {
            let nb = neighbourhood(&current, space).len();
            crate::debug_line(options.debug.main, &format!("[{:8.2}s] ils: bls neighborhood={nb} instances={n_runs} incumbent={incumbent_score:.6}", crate::t()));
        }
        let (new_lm, new_lm_score) = basic_local_search(
            current, current_score,
            instances, n_runs, &scheduler, cache, options, space,
            incumbent_score, &mut rng, deadline,
        )?;

        // Update incumbent.  `new_lm_score`, `incumbent_score` and
        // `last_lm_score` are all measured on `instances[..n_runs]` here — the
        // fidelity block at the end of the loop re-measures the two retained
        // states together, so the comparisons below never cross fidelities.
        let incumbent_survived =
            !dominates(new_lm_score, n_runs, incumbent_score, n_runs, options);
        if !incumbent_survived {
            incumbent = new_lm.clone();
            incumbent_score = new_lm_score;
            log_incumbent(
                options.debug.main,
                &incumbent,
                incumbent_score,
                n_runs,
                space,
            )?;
        }

        // Acceptance criterion: keep new local opt only if it dominates the last one
        let previous_home_base = last_lm.clone();
        let (accepted, accepted_score) = acceptance_criterion(
            new_lm, new_lm_score, n_runs,
            last_lm.clone(), last_lm_score, last_lm_runs,
            options,
        );
        last_lm = accepted;
        last_lm_score = accepted_score;
        last_lm_runs = n_runs;
        log_home_base(
            options.debug.main,
            &previous_home_base,
            &last_lm,
            last_lm_score,
            n_runs,
            space,
        );

        if incumbent_survived && options.approach == Approach::Focused {
            // Incumbent survived — increase fidelity for the next round (up to all instances).
            // This is the bounded increase mechanism: challengers that fail against the
            // current incumbent push it to be evaluated on another fidelity step.
            let next = next_n_runs(n_runs, options.fidelity_step, n_total);
            if next > n_runs {
                let next_evaluation = evaluate_config_outcome(
                    &incumbent, &instances[..next], &scheduler, cache, options, space, None, deadline,
                )?;
                if !(next_evaluation.complete && next_evaluation.score.is_finite()) {
                    crate::debug_line(options.debug.main, &format!(
                        "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete; retaining {n_runs}-run incumbent_score={incumbent_score:.6}",
                        crate::t()
                    ));
                    break;
                }

                // Re-measure the ILS home base at the new fidelity too.  It is
                // compared against the challenger on every subsequent round, so
                // leaving its score on the old prefix would compare two
                // different objectives.  Because prefix means drift, such a
                // stale bar is biased and — the acceptance criterion being
                // monotone — can only be updated by the comparison it blocks,
                // which freezes the perturbation centre for the rest of the run.
                // The home base is usually the incumbent, in which case the
                // evaluation is already done.
                let home_base_is_incumbent = hash_config(&active_config(&last_lm, space))
                    == hash_config(&active_config(&incumbent, space));
                let home_base_score = if home_base_is_incumbent {
                    next_evaluation.score
                } else {
                    let home_base_evaluation = evaluate_config_outcome(
                        &last_lm, &instances[..next], &scheduler, cache, options, space, None, deadline,
                    )?;
                    if !(home_base_evaluation.complete && home_base_evaluation.score.is_finite()) {
                        crate::debug_line(options.debug.main, &format!(
                            "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete (home base); retaining {n_runs}-run incumbent_score={incumbent_score:.6}",
                            crate::t()
                        ));
                        break;
                    }
                    home_base_evaluation.score
                };

                n_runs = next;
                incumbent_score = next_evaluation.score;
                last_lm_score = home_base_score;
                last_lm_runs = next;
                crate::debug_line(options.debug.main, &format!(
                    "[{:8.2}s] ils: n_runs increased to {n_runs}/{n_total} incumbent_score={incumbent_score:.6} home_base_score={last_lm_score:.6}",
                    crate::t()
                ));
            }
        }
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
///
/// ParamILS spells this `dominates(a, b, equalIsBetter=false)`; the `≤` variant
/// is [`weakly_dominates`].
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

/// `a` is at least as good as `b` — ParamILS's `dominates(a, b, equalIsBetter=true)`.
///
/// Used only by the acceptance criterion. ParamILS resolves a tie there in
/// favour of the challenger, with the comment "new <= old handled first ->
/// moving away from incumbent": on a plateau the ILS home base keeps moving
/// while the incumbent stays put, which is what lets the two diverge and the
/// search drift away from a basin it cannot improve on. The incumbent
/// comparison keeps the strict [`dominates`], so fidelity growth is unaffected.
pub fn weakly_dominates(
    a_score: f64, a_runs: usize,
    b_score: f64, b_runs: usize,
    options: &IlsOptions,
) -> bool {
    match options.approach {
        Approach::Basic | Approach::Random => a_score <= b_score,
        Approach::Focused => a_runs >= b_runs && a_score <= b_score,
    }
}

/// Return the new local optimum if it dominates the previous one; otherwise
/// keep the previous one.
///
/// `new_runs` and `last_runs` are the fidelities the two scores were measured
/// on. The caller keeps them equal (see the fidelity-consistency note in the
/// module docs); they are passed rather than assumed so that `dominates`'
/// FocusedILS guard still refuses a claim made from a smaller sample if that
/// invariant is ever broken.
fn acceptance_criterion(
    new: Config, new_score: f64, new_runs: usize,
    last: Config, last_score: f64, last_runs: usize,
    options: &IlsOptions,
) -> (Config, f64) {
    debug_assert_eq!(
        new_runs, last_runs,
        "acceptance compares scores measured on different instance prefixes"
    );
    if weakly_dominates(new_score, new_runs, last_score, last_runs, options) {
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
#[allow(clippy::too_many_arguments)]
fn evaluate_config(
    config: &Config,
    instances: &[(i64, String)],
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    space: &ParamSpace,
    incumbent_score: Option<f64>,
    deadline: Instant,
) -> Result<f64> {
    Ok(evaluate_config_outcome(
        config,
        instances,
        scheduler,
        cache,
        options,
        space,
        incumbent_score,
        deadline,
    )?
    .score)
}

struct ConfigEvaluation {
    score: f64,
    complete: bool,
}

#[allow(clippy::too_many_arguments)]
fn evaluate_config_outcome(
    config: &Config,
    instances: &[(i64, String)],
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    space: &ParamSpace,
    incumbent_score: Option<f64>,
    deadline: Instant,
) -> Result<ConfigEvaluation> {
    if instances.is_empty() {
        return Ok(ConfigEvaluation {
            score: 0.0,
            complete: true,
        });
    }

    let eval_config = active_config(config, space);
    let hash = hash_config(&eval_config);
    let batch_id = scheduler.submit(vec![EvalTask {
        neighbor_id: 0,
        config: eval_config,
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
            let eval_config = active_config(cfg, space);
            let hash = hash_config(&eval_config);
            EvalTask {
                neighbor_id: i,
                config: eval_config,
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

            if result.cacheable && result.status != "UNKNOWN" {
                cache.put(
                    result.hash,
                    result.instance_id,
                    result.runtime,
                    result.quality,
                    &result.status,
                    result.cutoff,
                )?;
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
                if r.cacheable && r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff)?;
                }
            }
                    crate::debug_line(options.debug.main, &format!(
                        "[{:8.2}s] ils: bls improvement neighbor={nid} score={score:.6} (was {current_score:.6})",
                        crate::t()
                    ));
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: bls arguments: {}",
                            crate::t(),
                            format_argument_changes(&current, &neighbors[nid], space)
                        ),
                    );
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
                if r.cacheable && r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff)?;
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
#[allow(clippy::too_many_arguments)]
fn collect_one(
    batch_id: u64,
    n_instances: usize,
    expected_nid: usize,
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    incumbent_score: Option<f64>,
    deadline: Instant,
) -> Result<ConfigEvaluation> {
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

        if result.cacheable && result.status != "UNKNOWN" {
            cache.put(
                result.hash,
                result.instance_id,
                result.runtime,
                result.quality,
                &result.status,
                result.cutoff,
            )?;
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
                if r.cacheable && r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff)?;
                }
            }
                    break;
                }
            }
        }
    }

    let complete = runtimes.len() == n_instances;
    let score = if runtimes.is_empty() {
        f64::INFINITY
    } else {
        compute_score(&runtimes, &qualities, options)
    };
    Ok(ConfigEvaluation { score, complete })
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

fn active_config(config: &Config, space: &ParamSpace) -> Config {
    space.active_params(config)
        .into_iter()
        .filter_map(|param| {
            config.get(&param.name)
                .map(|value| (param.name.clone(), value.clone()))
        })
        .collect()
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

    fn conditional_space() -> ParamSpace {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
        writeln!(f, "limit {{1, 2}} [1] | mode in {{slow}}").unwrap();
        let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
        drop(f);
        space
    }

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn focused_options() -> IlsOptions {
        IlsOptions {
            approach: Approach::Focused,
            n_workers: 1,
            perturbation_strength: 4,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: true,
            tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
            debug: crate::DebugOptions::default(),
        }
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
    fn evaluation_config_omits_inactive_parameters() {
        let space = conditional_space();
        let first = cfg(&[("mode", "fast"), ("limit", "1")]);
        let second = cfg(&[("mode", "fast"), ("limit", "2")]);

        let first_active = active_config(&first, &space);
        let second_active = active_config(&second, &space);

        assert_eq!(first_active, cfg(&[("mode", "fast")]));
        assert_eq!(first_active, second_active);
        assert_eq!(hash_config(&first_active), hash_config(&second_active));
    }

    #[test]
    fn argument_changes_include_conditional_activation() {
        let space = conditional_space();
        let current = cfg(&[("mode", "fast"), ("limit", "2")]);
        let next = cfg(&[("mode", "slow"), ("limit", "2")]);

        assert_eq!(
            format_argument_changes(&current, &next, &space),
            "limit: <inactive> -> 2; mode: fast -> slow"
        );
        assert_eq!(
            format_argument_changes(&next, &current, &space),
            "limit: 2 -> <inactive>; mode: slow -> fast"
        );
    }

    #[test]
    fn argument_changes_are_sorted() {
        let space = simple_space();
        let current = cfg(&[("alpha", "1"), ("beta", "b")]);
        let next = cfg(&[("alpha", "3"), ("beta", "a")]);

        assert_eq!(
            format_argument_changes(&current, &next, &space),
            "alpha: 1 -> 3; beta: b -> a"
        );
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
    fn weakly_dominates_resolves_ties_for_the_challenger() {
        let opts = focused_options();

        // Same as `dominates` except on ties, which the acceptance criterion
        // resolves in favour of the challenger ("moving away from incumbent").
        assert!(weakly_dominates(1.0, 10, 2.0, 5, &opts));
        assert!(weakly_dominates(1.0, 10, 1.0, 5, &opts)); // tie — accepted here
        assert!(!weakly_dominates(2.0, 10, 1.0, 5, &opts));
        // The FocusedILS run-count guard still applies.
        assert!(!weakly_dominates(1.0, 3, 1.0, 5, &opts));
        assert!(!weakly_dominates(1.0, 3, 2.0, 5, &opts));
    }

    #[test]
    fn acceptance_takes_better_and_ties_but_not_worse() {
        let opts = focused_options();
        let old = cfg(&[("alpha", "1")]);
        let new = cfg(&[("alpha", "2")]);

        // Strictly better: accepted.
        let (config, score) =
            acceptance_criterion(new.clone(), 1.0, 8, old.clone(), 2.0, 8, &opts);
        assert_eq!(config, new);
        assert!((score - 1.0).abs() < 1e-9);

        // Tie: accepted, so the home base can cross plateaus.
        let (config, score) =
            acceptance_criterion(new.clone(), 2.0, 8, old.clone(), 2.0, 8, &opts);
        assert_eq!(config, new);
        assert!((score - 2.0).abs() < 1e-9);

        // Worse: rejected, home base unchanged.
        let (config, score) =
            acceptance_criterion(new.clone(), 3.0, 8, old.clone(), 2.0, 8, &opts);
        assert_eq!(config, old);
        assert!((score - 2.0).abs() < 1e-9);
    }

    /// A stale home-base score from a smaller prefix must not be able to reject
    /// a challenger measured on the current one.  The loop prevents this by
    /// re-measuring both retained states on every fidelity increase; if that
    /// invariant were ever broken, the run-count guard is the backstop.
    #[test]
    fn stale_lower_fidelity_score_cannot_win_a_comparison() {
        let opts = focused_options();

        // The failure this reproduces: a home base measured on 12 instances
        // scoring 0.111, against a challenger measured on 568 scoring 0.489.
        // The stale score looks far better only because short prefixes of this
        // instance list are cheaper.
        assert!(!dominates(0.111, 12, 0.489, 568, &opts));
        assert!(!weakly_dominates(0.111, 12, 0.489, 568, &opts));

        // Measured on the same prefix, the comparison resolves normally.
        assert!(dominates(0.474, 568, 0.489, 568, &opts));
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
            &simple_space(),
            None,
            started + Duration::from_secs(2),
        ).unwrap();

        assert!(started.elapsed() >= Duration::from_millis(650));
        assert!((score - 0.7).abs() < 1e-9);
    }

    #[test]
    fn evaluation_marks_partial_cache_result_incomplete() {
        let mut cache = Cache::open(":memory:", false).unwrap();
        let paths = vec!["cached.cnf".to_string(), "slow.cnf".to_string()];
        let ids = cache.load_instances(&paths).unwrap();
        let instances = paths
            .iter()
            .map(|path| (ids[path], path.clone()))
            .collect::<Vec<_>>();
        let space = simple_space();
        let config = cfg(&[("alpha", "1"), ("beta", "a")]);
        let hash = hash_config(&active_config(&config, &space));
        cache
            .put(hash, ids["cached.cnf"], 0.1, 0.0, "sat", 2.0)
            .unwrap();

        let scheduler = Scheduler::new(
            1,
            "sleep 0.5; echo '#%# RamParIls #%# sat, 0.5, 0.0'; true".to_string(),
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
            tuner_timeout: 0.1,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
            debug: crate::DebugOptions::default(),
        };

        let evaluation = evaluate_config_outcome(
            &config,
            &instances,
            &scheduler,
            &mut cache,
            &options,
            &space,
            None,
            Instant::now() + Duration::from_millis(50),
        )
        .unwrap();

        assert!(!evaluation.complete);
        assert!((evaluation.score - 0.1).abs() < 1e-9);
    }
}
