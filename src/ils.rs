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

fn log_incumbent(enabled: bool, incumbent: &Config, eval: &ConfigEvaluation, n_runs: usize, space: &ParamSpace) -> Result<()> {
    if !enabled {
        return Ok(());
    }
    let hash = hash_config(&active_config(incumbent, space));
    let score = eval.score;
    let runhash = eval.runhash_suffix(n_runs);
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new incumbent: hash={hash:016x} score={score:.6} instances={n_runs}{runhash}",
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
fn log_home_base(enabled: bool, previous: &Config, home_base: &Config, score: &ConfigEvaluation, n_runs: usize, space: &ParamSpace) {
    if !enabled {
        return;
    }
    let changes = format_argument_changes(previous, home_base, space);
    if changes.is_empty() {
        return;
    }
    let hash = hash_config(&active_config(home_base, space));
    let runhash = score.runhash_suffix(n_runs);
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new home base: hash={hash:016x} score={} instances={n_runs}{runhash} changes: {changes}",
            crate::t(),
            score.display(n_runs)
        ),
    );
}

/// Per-run counters behind the end-of-run `ils: summary` line.
///
/// Globals rather than a `&mut Stats` threaded through six signatures: they are
/// bumped on the hot path from three different functions, and one process runs
/// one tuning run. [`run`] resets them on entry.
mod counters {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    pub static EVALS: AtomicUsize = AtomicUsize::new(0);
    pub static CAPPED: AtomicUsize = AtomicUsize::new(0);

    pub fn reset() {
        EVALS.store(0, Relaxed);
        CAPPED.store(0, Relaxed);
    }
    /// One configuration evaluated to a verdict — completed or capped. A
    /// neighbour cancelled mid-flight because another one improved first
    /// reached no verdict and is deliberately not counted.
    pub fn eval(capped: bool) {
        EVALS.fetch_add(1, Relaxed);
        if capped {
            CAPPED.fetch_add(1, Relaxed);
        }
    }
    pub fn get() -> (usize, usize) {
        (EVALS.load(Relaxed), CAPPED.load(Relaxed))
    }
}

fn format_argument_changes(current: &Config, next: &Config, space: &ParamSpace) -> String {
    let current = active_config(current, space);
    let next = active_config(next, space);
    let names: BTreeSet<&str> = current.keys().chain(next.keys()).map(String::as_str).collect();

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

/// Where a restart puts the ILS home base.
///
/// ParamILS only has [`RestartTarget::Random`] — its `init_random()`. Landing
/// on a uniformly random configuration is the strongest possible
/// diversification, which suits ParamILS's thousands of iterations but not a
/// run that gets a few dozen: the descent that follows starts from a
/// configuration that is almost certainly terrible, and it has to be paid for
/// out of the same budget. [`RestartTarget::Incumbent`] keeps the jump
/// anchored at the best configuration found so far, trading diversification
/// for not throwing away what the run already knows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RestartTarget {
    /// Perturb the incumbent by `restart_strength` steps.
    Incumbent,
    /// Draw a uniformly random configuration.
    Random,
}

/// Why a restart fired — recorded in the log so the two triggers can be told
/// apart when reading a run back.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RestartReason {
    /// `restart_failures` consecutive local optima were rejected.
    Stagnation,
    /// The `restart_probability` coin came up.
    Probability,
}

impl RestartReason {
    fn as_str(self) -> &'static str {
        match self {
            RestartReason::Stagnation => "stagnation",
            RestartReason::Probability => "probability",
        }
    }
}

/// All tunable settings for a single ILS run.
#[derive(Clone)]
pub struct IlsOptions {
    pub approach: Approach,
    /// Max parallel worker threads.
    pub n_workers: usize,
    /// Number of neighbourhood steps for perturbation.
    pub perturbation_strength: usize,
    /// Probability of restarting the home base after a round (ParamILS
    /// `p_restart`); 0 disables it.
    pub restart_probability: f64,
    /// Restart the home base after this many consecutive rejected local
    /// optima; 0 disables it.  Both triggers may be enabled at once.
    pub restart_failures: usize,
    /// Where a restart puts the home base.
    pub restart_target: RestartTarget,
    /// Perturbation steps a restart applies to the incumbent.  Already
    /// resolved: callers substitute `2 * perturbation_strength` for 0.
    pub restart_strength: usize,
    /// Relative margin around the *incumbent* within which a worse local
    /// optimum is still accepted as the new home base; 0 disables it.
    pub acceptance_tolerance: f64,
    /// ParamILS's `R`: how many random configurations to probe before the
    /// first descent, keeping any that beats the starting point.  0 (the
    /// default) starts from the supplied configuration and nothing else.
    pub random_probes: usize,
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
    counters::reset();
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
            Approach::Basic => "basic",
            Approach::Focused => "focused",
            Approach::Random => "random",
        };
        crate::debug_line(
            d,
            &format!(
                "[{t:8.2}s] ils: starting approach={approach_str} instances={n_total} timeout={:.0}s",
                options.tuner_timeout
            ),
        );
        crate::debug_line(d, &format!("[{t:8.2}s] ils: initial config:"));
        match &initial {
            Some(cfg) => crate::debug_block(d, &config_to_yaml(cfg)?),
            None => crate::debug_line(d, "           (random)"),
        }
    }
    // Without a starting configuration there is nothing to start from but a
    // random draw.  Any further sampling is `random_probes` below, so that
    // there is one rule for how many random configurations a run looks at
    // rather than two paths that disagree.
    let mut current = match initial {
        Some(cfg) => cfg,
        None => random_config(space, &mut rng),
    };

    // `None` for the bound: nothing is known yet to prune against, so this one
    // is always complete.
    let mut current_eval = evaluate_config_outcome(
        &current,
        &instances[..n_runs],
        &scheduler,
        cache,
        options,
        space,
        None,
        deadline,
    )?;

    // ParamILS's `R` random probes: sample configurations and step to any that
    // beats the starting point, before the first descent commits to a region.
    // ParamILS does this on every run, including when a default configuration
    // was supplied.  Here it defaults to 0, because the primary use is
    // specializing a strategy handed in by Grackle: the caller's configuration
    // is the point of the run, and probing away from it by default would
    // defeat that.
    for _ in 0..options.random_probes {
        if Instant::now() >= deadline || crate::interrupted() {
            break;
        }
        let probe = random_config(space, &mut rng);
        let probe_score = evaluate_config(
            &probe,
            &instances[..n_runs],
            &scheduler,
            cache,
            options,
            space,
            Some(current_eval.score),
            deadline,
        )?;
        // A capped probe scores above the bound and so above `current`, and
        // cannot dominate: anything that gets through here ran to completion.
        if dominates(probe_score, n_runs, current_eval.score, n_runs, options) {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: random probe improves: score={probe_score:.6} (was {}) instances={n_runs}",
                    crate::t(),
                    current_eval.display(n_runs)
                ),
            );
            current = probe;
            current_eval = ConfigEvaluation::complete(probe_score, n_runs);
        }
    }

    let mut incumbent = current.clone();
    let mut incumbent_score = current_eval.score;
    // Kept for the home-base diff below: `incumbent` may be replaced by the
    // first descent, and `current` is moved into it.
    let initial_config = current.clone();

    // --- First BLS ---
    let (lm, lm_eval, first_steps) = basic_local_search(
        current,
        current_eval,
        instances,
        n_runs,
        &scheduler,
        cache,
        options,
        space,
        incumbent_score,
        &mut rng,
        deadline,
    )?;
    // Counts `ils: new incumbent` lines — replacements of the starting one.
    let mut n_incumbents = 0usize;
    if dominates(lm_eval.score, n_runs, incumbent_score, n_runs, options) {
        incumbent = lm.clone();
        incumbent_score = lm_eval.score;
        n_incumbents += 1;
        log_incumbent(options.debug.main, &incumbent, &lm_eval, n_runs, space)?;
    }
    let mut last_lm = lm;
    let mut last_lm_eval = lm_eval;
    // Fidelity at which `last_lm_eval` was measured; kept equal to `n_runs`.
    let mut last_lm_runs = n_runs;
    log_home_base(
        options.debug.main,
        &initial_config,
        &last_lm,
        &last_lm_eval,
        n_runs,
        space,
    );

    // Round accounting for the end-of-run summary.  The first descent above
    // counts as a round: it is a descent, and it can never be gated, which is
    // itself worth seeing in the ratio.
    let mut n_rounds = 1usize;
    let mut n_searched = usize::from(first_steps > 0);
    let mut n_gated = 0usize;

    // Consecutive rounds whose local optimum failed the acceptance criterion,
    // which is what `restart_failures` counts.
    let mut rejections = 0usize;

    // --- Main ILS loop ---
    while Instant::now() < deadline && !crate::interrupted() {
        // Perturbation.  `Approach::Random` is ParamILS's `pert_rand`: replace
        // the perturbation with a fresh random configuration and drop the
        // acceptance criterion entirely, which makes the run a random-restart
        // baseline rather than an iterated local search.
        let perturbed = if options.approach == Approach::Random {
            crate::debug_line(
                options.debug.main,
                &format!("[{:8.2}s] ils: random restart (approach=random)", crate::t()),
            );
            random_config(space, &mut rng)
        } else {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: perturbation strength={}",
                    crate::t(),
                    options.perturbation_strength
                ),
            );
            perturbation(last_lm.clone(), options.perturbation_strength, space, &mut rng)
        };
        current = perturbed;
        current_eval = evaluate_config_outcome(
            &current,
            &instances[..n_runs],
            &scheduler,
            cache,
            options,
            space,
            Some(incumbent_score),
            deadline,
        )?;
        // A capped start is what gates a round: every neighbour that does not
        // finish the whole set under the bound is invisible from here, so the
        // descent may never get going at all.
        let gated_start = !current_eval.complete;

        if Instant::now() >= deadline || crate::interrupted() {
            break;
        }

        // BLS from the perturbed point — evaluate neighbours on n_runs instances
        if options.debug.main {
            let nb = neighbourhood(&current, space).len();
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: bls neighborhood={nb} instances={n_runs} incumbent={incumbent_score:.6}",
                    crate::t()
                ),
            );
        }
        let (new_lm, new_lm_eval, steps) = basic_local_search(
            current,
            current_eval,
            instances,
            n_runs,
            &scheduler,
            cache,
            options,
            space,
            incumbent_score,
            &mut rng,
            deadline,
        )?;
        n_rounds += 1;
        if steps > 0 {
            n_searched += 1;
        } else if gated_start {
            n_gated += 1;
        }

        // Update incumbent.  `new_lm_eval`, `incumbent_score` and
        // `last_lm_eval` are all measured on `instances[..n_runs]` here — the
        // fidelity block at the end of the loop re-measures the two retained
        // states together, so the comparisons below never cross fidelities.
        let incumbent_survived = !dominates(new_lm_eval.score, n_runs, incumbent_score, n_runs, options);
        if !incumbent_survived {
            incumbent = new_lm.clone();
            incumbent_score = new_lm_eval.score;
            n_incumbents += 1;
            log_incumbent(options.debug.main, &incumbent, &new_lm_eval, n_runs, space)?;
        }

        // Acceptance criterion: keep new local opt only if it dominates the
        // last one.  Skipped entirely under `Approach::Random`, where the next
        // round starts from a fresh random configuration regardless — there is
        // no home base to keep, and a restart would be a no-op.
        if options.approach == Approach::Random {
            let previous_home_base = last_lm.clone();
            last_lm = new_lm;
            last_lm_eval = new_lm_eval;
            last_lm_runs = n_runs;
            log_home_base(
                options.debug.main,
                &previous_home_base,
                &last_lm,
                &last_lm_eval,
                n_runs,
                space,
            );
            continue;
        }

        let previous_home_base = last_lm.clone();
        let (accepted, accepted_score, took_new) = acceptance_criterion(
            new_lm,
            new_lm_eval.score,
            n_runs,
            last_lm.clone(),
            last_lm_eval.score,
            last_lm_runs,
            incumbent_score,
            options,
        );
        last_lm = accepted;
        last_lm_eval = if took_new { new_lm_eval } else { last_lm_eval };
        debug_assert_eq!(last_lm_eval.score, accepted_score);
        last_lm_runs = n_runs;
        log_home_base(
            options.debug.main,
            &previous_home_base,
            &last_lm,
            &last_lm_eval,
            n_runs,
            space,
        );

        // Restart.  The acceptance criterion above can only move the home base
        // to an at-least-as-good local optimum (or, with a tolerance, one
        // close to the incumbent), so on its own it can never move the search
        // uphill: a home base that stops improving stays put for the rest of
        // the budget and every later round perturbs the same point.  Either
        // trigger below breaks that.
        rejections = if took_new { 0 } else { rejections + 1 };
        let reason = if options.restart_failures > 0 && rejections >= options.restart_failures {
            Some(RestartReason::Stagnation)
        } else if options.restart_probability > 0.0 && rng.gen_range(0.0..1.0) < options.restart_probability {
            Some(RestartReason::Probability)
        } else {
            None
        };

        if let Some(reason) = reason {
            if Instant::now() >= deadline || crate::interrupted() {
                break;
            }
            let restarted = match options.restart_target {
                RestartTarget::Incumbent => perturbation(incumbent.clone(), options.restart_strength, space, &mut rng),
                RestartTarget::Random => random_config(space, &mut rng),
            };
            // Capped against the incumbent: a restart lands on a configuration
            // that is usually much worse, and there is no reason to pay for a
            // full evaluation of one.  A pruned evaluation yields a score above
            // the cap, which is exactly the "bad home base" the next round is
            // meant to escape from anyway.
            let restarted_eval = evaluate_config_outcome(
                &restarted,
                &instances[..n_runs],
                &scheduler,
                cache,
                options,
                space,
                Some(incumbent_score),
                deadline,
            )?;
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: restart: reason={} target={} strength={} score={} instances={n_runs} after {rejections} rejected local optima",
                    crate::t(),
                    reason.as_str(),
                    match options.restart_target {
                        RestartTarget::Incumbent => "incumbent",
                        RestartTarget::Random => "random",
                    },
                    match options.restart_target {
                        RestartTarget::Incumbent => options.restart_strength,
                        RestartTarget::Random => 0,
                    },
                    restarted_eval.display(n_runs),
                ),
            );
            let before_restart = last_lm.clone();
            last_lm = restarted;
            last_lm_eval = restarted_eval;
            last_lm_runs = n_runs;
            rejections = 0;
            log_home_base(
                options.debug.main,
                &before_restart,
                &last_lm,
                &last_lm_eval,
                n_runs,
                space,
            );
        }

        if incumbent_survived && options.approach == Approach::Focused {
            // Incumbent survived — increase fidelity for the next round (up to all instances).
            // This is the bounded increase mechanism: challengers that fail against the
            // current incumbent push it to be evaluated on another fidelity step.
            let next = next_n_runs(n_runs, options.fidelity_step, n_total);
            if next > n_runs {
                let next_evaluation = evaluate_config_outcome(
                    &incumbent,
                    &instances[..next],
                    &scheduler,
                    cache,
                    options,
                    space,
                    None,
                    deadline,
                )?;
                if !(next_evaluation.complete && next_evaluation.score.is_finite()) {
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete; retaining {n_runs}-run incumbent_score={incumbent_score:.6}",
                            crate::t()
                        ),
                    );
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
                let home_base_is_incumbent =
                    hash_config(&active_config(&last_lm, space)) == hash_config(&active_config(&incumbent, space));
                // Carry the full evaluation (runhash included) rather than
                // just its score, so `last_lm_eval` below keeps that data
                // instead of losing it to a bare `ConfigEvaluation::complete`.
                let home_base_evaluation = if home_base_is_incumbent {
                    next_evaluation
                } else {
                    let home_base_evaluation = evaluate_config_outcome(
                        &last_lm,
                        &instances[..next],
                        &scheduler,
                        cache,
                        options,
                        space,
                        None,
                        deadline,
                    )?;
                    if !(home_base_evaluation.complete && home_base_evaluation.score.is_finite()) {
                        crate::debug_line(
                            options.debug.main,
                            &format!(
                                "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete (home base); retaining {n_runs}-run incumbent_score={incumbent_score:.6}",
                                crate::t()
                            ),
                        );
                        break;
                    }
                    home_base_evaluation
                };

                n_runs = next;
                incumbent_score = next_evaluation.score;
                // Both re-measurements above are guarded on `complete`, so the
                // home base's new score is a full one at the new fidelity.
                let home_base_score = home_base_evaluation.score;
                last_lm_eval = home_base_evaluation;
                last_lm_runs = next;
                crate::debug_line(
                    options.debug.main,
                    &format!(
                        "[{:8.2}s] ils: n_runs increased to {n_runs}/{n_total} incumbent_score={incumbent_score:.6} home_base_score={home_base_score:.6}",
                        crate::t()
                    ),
                );
            }
        }
    }

    let (evals, capped) = counters::get();
    crate::debug_line(
        options.debug.main,
        &format!(
            "[{:8.2}s] ils: summary rounds={n_rounds} searched={n_searched} gated={n_gated} \
             incumbents={n_incumbents} evals={evals} capped={capped}",
            crate::t()
        ),
    );

    Ok((incumbent, incumbent_score))
}

// ---------------------------------------------------------------------------
// Core algorithm functions
// ---------------------------------------------------------------------------

/// All one-parameter-away neighbours of `config` within `space`.
/// Only iterates over active params; skips forbidden combinations, tested
/// against each candidate's active projection (see `random_config`).
pub fn neighbourhood(config: &Config, space: &ParamSpace) -> Vec<Config> {
    let active = space.active_params(config);
    let mut result = Vec::new();
    let empty = String::new();
    for param in active {
        let current_val = config.get(&param.name).unwrap_or(&empty);
        for value in &param.domain {
            if value == current_val {
                continue;
            }
            let mut new_cfg = config.clone();
            new_cfg.insert(param.name.clone(), value.clone());
            if !space.is_forbidden(&active_config(&new_cfg, space)) {
                result.push(new_cfg);
            }
        }
    }
    result
}

/// Random walk: take `strength` steps through the neighbourhood.
pub fn perturbation(config: Config, strength: usize, space: &ParamSpace, rng: &mut impl Rng) -> Config {
    if matches!(strength, 0) {
        return config;
    }
    let mut current = config;
    for _ in 0..strength {
        let neighbors = neighbourhood(&current, space);
        if neighbors.is_empty() {
            break;
        }
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
pub fn dominates(a_score: f64, a_runs: usize, b_score: f64, b_runs: usize, options: &IlsOptions) -> bool {
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
pub fn weakly_dominates(a_score: f64, a_runs: usize, b_score: f64, b_runs: usize, options: &IlsOptions) -> bool {
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
///
/// With `acceptance_tolerance > 0` a *worse* local optimum can still be
/// accepted, provided it stays within that relative margin of the incumbent.
/// The margin deliberately hangs off the incumbent and not off the home base:
/// against the home base each accepted step raises the bar for the next one, so
/// the home base can walk downhill indefinitely in small increments. Against
/// the incumbent the home base is confined to a fixed band around the best
/// score seen, which is what lets the search leave a basin without abandoning
/// the region it already knows is good.
///
/// The third return value reports whether the new local optimum was taken —
/// the caller counts consecutive rejections to drive the stagnation restart.
#[allow(clippy::too_many_arguments)]
fn acceptance_criterion(
    new: Config,
    new_score: f64,
    new_runs: usize,
    last: Config,
    last_score: f64,
    last_runs: usize,
    incumbent_score: f64,
    options: &IlsOptions,
) -> (Config, f64, bool) {
    debug_assert_eq!(
        new_runs, last_runs,
        "acceptance compares scores measured on different instance prefixes"
    );
    if weakly_dominates(new_score, new_runs, last_score, last_runs, options) {
        return (new, new_score, true);
    }
    if accepted_within_tolerance(new_score, incumbent_score, options) {
        return (new, new_score, true);
    }
    (last, last_score, false)
}

/// Whether `new_score` is close enough to the incumbent to be accepted as the
/// home base despite being worse than the current one.
///
/// The band is `incumbent + tolerance * |incumbent|` rather than
/// `incumbent * (1 + tolerance)` so that it still widens in the right direction
/// when the quality objective produces negative scores.
fn accepted_within_tolerance(new_score: f64, incumbent_score: f64, options: &IlsOptions) -> bool {
    if options.acceptance_tolerance <= 0.0 {
        return false;
    }
    if !new_score.is_finite() || !incumbent_score.is_finite() {
        return false;
    }
    new_score <= incumbent_score + options.acceptance_tolerance * incumbent_score.abs()
}

// ---------------------------------------------------------------------------
// Evaluation helpers
// ---------------------------------------------------------------------------

/// Evaluate `config` on all instances in parallel.  Returns the scalar score.
///
/// Cache hits are served immediately; misses are dispatched to worker threads.
/// Adaptive capping prunes as soon as the running sum exceeds the budget
/// `bound_multiplier × incumbent_score × n_instances`.
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

/// A score, and whether it is one.
///
/// `score` is a mean over `n_done` instances. When `complete` is false, adaptive
/// capping stopped the evaluation early and those `n_done` are the ones that
/// happened to finish first — the fastest — so the mean **understates** the true
/// mean and the real score is worse. A capped score is therefore a lower bound,
/// not a measurement, and [`display`](ConfigEvaluation::display) renders it as
/// `>2.698475 (312/473)` so a log can never be read as if it were the latter.
#[derive(Clone, Copy)]
struct ConfigEvaluation {
    score: f64,
    complete: bool,
    n_done: usize,
    /// XOR of the `runhash` of every contributing instance that terminated
    /// (see `eval::TaskResult::runhash`). Meaningless when `runhash_n == 0`.
    runhash: u64,
    /// How many instances contributed to `runhash` — always `<= n_done`,
    /// since a timeout/error/unknown result carries no runhash. Comparing two
    /// evaluations' `runhash` is only informative when their `runhash_n`
    /// agree; a partial batch is measured over a different set of instances.
    runhash_n: usize,
}

impl ConfigEvaluation {
    /// A complete evaluation with no runhash data (e.g. a random probe, whose
    /// caller only kept the scalar score).
    fn complete(score: f64, n_done: usize) -> Self {
        Self { score, complete: true, n_done, runhash: 0, runhash_n: 0 }
    }

    /// `2.698475` when complete, `>2.698475 (312/473)` when capped.
    fn display(&self, n_instances: usize) -> String {
        if self.complete {
            format!("{:.6}", self.score)
        } else {
            format!(">{:.6} ({}/{})", self.score, self.n_done, n_instances)
        }
    }

    /// `runhash=<hex> (n=<runhash_n>/<n_instances>)`, or empty when nothing
    /// contributed — appended to a log line beside `display()`.
    fn runhash_suffix(&self, n_instances: usize) -> String {
        if self.runhash_n == 0 {
            String::new()
        } else {
            format!(" runhash={:016x} (n={}/{n_instances})", self.runhash, self.runhash_n)
        }
    }
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
        return Ok(ConfigEvaluation::complete(0.0, 0));
    }

    let eval_config = active_config(config, space);
    let hash = hash_config(&eval_config);
    let batch_id = scheduler.submit(
        vec![EvalTask {
            neighbor_id: 0,
            config: eval_config,
            hash,
            instances: Arc::new(instances.to_vec()),
        }],
        cache,
    )?;

    collect_one(
        batch_id,
        instances.len(),
        0,
        scheduler,
        cache,
        options,
        incumbent_score,
        deadline,
    )
}

/// Parallel first-improvement BLS.
///
/// Submits all neighbours as `EvalTask`s at once.  Accepts the first
/// fully-evaluated neighbour that dominates the current config (in
/// evaluation-completion order).  Resets the scheduler when a better
/// neighbour is found (so we don't wait for the rest).
/// Returns the local optimum, its evaluation, and how many moves were accepted
/// getting there.  A step count of zero on a capped start is a *gated* round:
/// the descent never moved because nothing in the neighbourhood could be seen.
fn basic_local_search(
    start: Config,
    start_eval: ConfigEvaluation,
    instances: &[(i64, String)],
    n_runs: usize,
    scheduler: &Scheduler,
    cache: &mut Cache,
    options: &IlsOptions,
    space: &ParamSpace,
    incumbent_score: f64,
    rng: &mut impl Rng,
    deadline: Instant,
) -> Result<(Config, ConfigEvaluation, usize)> {
    let eval_instances = &instances[..n_runs];
    let n_instances = n_runs;
    let mut current = start;
    // Only ever replaced by a neighbour that ran to completion, so after the
    // first accepted step this is a real score even if `start_eval` was capped.
    let mut current_eval = start_eval;
    let mut steps = 0usize;
    let mut changed = true;

    while changed && Instant::now() < deadline && !crate::interrupted() {
        changed = false;

        let mut neighbors = neighbourhood(&current, space);
        if neighbors.is_empty() {
            break;
        }

        // Shuffle for random first-improvement ordering
        for i in (1..neighbors.len()).rev() {
            let j = rng.gen_range(0..=i);
            neighbors.swap(i, j);
        }

        let n = neighbors.len();

        // Submit all neighbours (evaluated on the first n_runs instances only)
        let shared_instances = Arc::new(eval_instances.to_vec());
        let tasks: Vec<EvalTask> = neighbors
            .iter()
            .enumerate()
            .map(|(i, cfg)| {
                let eval_config = active_config(cfg, space);
                let hash = hash_config(&eval_config);
                EvalTask {
                    neighbor_id: i,
                    config: eval_config,
                    hash,
                    instances: Arc::clone(&shared_instances),
                }
            })
            .collect();
        let batch_id = scheduler.submit(tasks, cache)?;

        // Per-neighbour tracking
        let mut runtimes: Vec<Vec<f64>> = vec![vec![]; n];
        let mut qualities: Vec<Vec<f64>> = vec![vec![]; n];
        let mut partial: Vec<f64> = vec![0.0; n];
        let mut runhashes: Vec<u64> = vec![0; n];
        let mut runhash_ns: Vec<usize> = vec![0; n];
        let mut done = vec![false; n];
        let mut n_done = 0usize;

        'collect: loop {
            if n_done >= n {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || crate::interrupted() {
                break;
            }

            let result = match scheduler
                .results()
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
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
                    result.runhash,
                )?;
            }
            if result.batch_id != batch_id {
                continue;
            }

            let nid = result.neighbor_id;
            // Guard against stale results from a previous reset (shouldn't
            // normally happen, but the window between reset() and drain is tiny).
            if nid >= n || done[nid] {
                continue;
            }

            runtimes[nid].push(result.runtime);
            qualities[nid].push(result.quality);
            if let Some(h) = result.runhash {
                runhashes[nid] ^= h;
                runhash_ns[nid] += 1;
            }
            let val = match options.run_obj {
                RunObjective::Runtime => result.runtime,
                RunObjective::Quality => result.quality,
            };
            partial[nid] += val;

            // Adaptive capping: prune this neighbour once it has spent the whole
            // budget that beating the incumbent allows. Costs never go down, so
            // passing the budget *proves* the final mean exceeds the bound —
            // this is a decision, not a guess, and it is the earliest point at
            // which the proof exists.
            if options.pruning {
                let budget = options.bound_multiplier * incumbent_score * n_instances as f64;
                if partial[nid] > budget {
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: capped neighbor={nid} spent={:.6} budget={budget:.6} after {}/{n_instances}",
                            crate::t(),
                            partial[nid],
                            runtimes[nid].len(),
                        ),
                    );
                    done[nid] = true;
                    n_done += 1;
                    counters::eval(true);
                    continue;
                }
            }

            if runtimes[nid].len() == n_instances {
                done[nid] = true;
                n_done += 1;
                counters::eval(false);
                let score = compute_score(&runtimes[nid], &qualities[nid], options);

                if dominates(score, n_instances, current_eval.score, n_instances, options) {
                    // Accept — stop evaluating the rest
                    scheduler.reset();
                    while let Ok(r) = scheduler.results().try_recv() {
                        if r.cacheable && r.status != "UNKNOWN" {
                            cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff, r.runhash)?;
                        }
                    }
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: bls improvement neighbor={nid} score={score:.6} (was {})",
                            crate::t(),
                            current_eval.display(n_instances)
                        ),
                    );
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: bls arguments: {}",
                            crate::t(),
                            format_argument_changes(&current, &neighbors[nid], space)
                        ),
                    );
                    current = neighbors[nid].clone();
                    current_eval = ConfigEvaluation {
                        score,
                        complete: true,
                        n_done: n_instances,
                        runhash: runhashes[nid],
                        runhash_n: runhash_ns[nid],
                    };
                    steps += 1;
                    changed = true;
                    break 'collect;
                }
            }
        }

        if !changed {
            scheduler.reset();
            while let Ok(r) = scheduler.results().try_recv() {
                if r.cacheable && r.status != "UNKNOWN" {
                    cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff, r.runhash)?;
                }
            }
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: bls local optimum score={}",
                    crate::t(),
                    current_eval.display(n_instances)
                ),
            );
        }
    }

    Ok((current, current_eval, steps))
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
    let mut runhash = 0u64;
    let mut runhash_n = 0usize;

    while runtimes.len() < n_instances {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || crate::interrupted() {
            break;
        }

        let result = match scheduler
            .results()
            .recv_timeout(remaining.min(Duration::from_millis(500)))
        {
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
                result.runhash,
            )?;
        }
        if result.batch_id != batch_id {
            continue;
        }
        if result.neighbor_id != expected_nid {
            continue;
        }

        let val = match options.run_obj {
            RunObjective::Runtime => result.runtime,
            RunObjective::Quality => result.quality,
        };
        partial_sum += val;
        runtimes.push(result.runtime);
        qualities.push(result.quality);
        if let Some(h) = result.runhash {
            runhash ^= h;
            runhash_n += 1;
        }

        if options.pruning {
            if let Some(inc) = incumbent_score {
                // Same budget test as the neighbour loop above; see there.
                if partial_sum > options.bound_multiplier * inc * n_instances as f64 {
                    scheduler.reset();
                    while let Ok(r) = scheduler.results().try_recv() {
                        if r.cacheable && r.status != "UNKNOWN" {
                            cache.put(r.hash, r.instance_id, r.runtime, r.quality, &r.status, r.cutoff, r.runhash)?;
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
    counters::eval(!complete);
    Ok(ConfigEvaluation { score, complete, n_done: runtimes.len(), runhash, runhash_n })
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
            if n % 2 == 0 {
                (s[n / 2 - 1] + s[n / 2]) / 2.0
            } else {
                s[n / 2]
            }
        }
    }
}

fn active_config(config: &Config, space: &ParamSpace) -> Config {
    space
        .active_params(config)
        .into_iter()
        .filter_map(|param| config.get(&param.name).map(|value| (param.name.clone(), value.clone())))
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
    let schedule: Vec<(usize, f64, f64)> = (0..num_depths)
        .map(|i| {
            let exp = (num_depths - 1 - i) as f64;
            let n = ((n_total as f64) * lambda_n.powf(exp)).ceil() as usize;
            let n = n.max(1).min(n_total);
            let c = (cutoff_time * lambda_c.powf(exp)).ceil();
            let t = options.tuner_timeout * lambda_t.powf(exp);
            (n, c, t)
        })
        .collect();

    if options.debug.main {
        crate::debug_line(
            options.debug.main,
            &format!(
                "[{:8.2}s] id: {} phases  λ_n={lambda_n} λ_c={lambda_c} λ_t={lambda_t}",
                crate::t(),
                num_depths
            ),
        );
        for (i, (n, c, t)) in schedule.iter().enumerate() {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id:   phase {} n={n} cutoff={c:.1}s timeout={t:.1}s",
                    crate::t(),
                    i + 1
                ),
            );
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
                crate::debug_line(
                    options.debug.main,
                    &format!(
                        "[{:8.2}s] id: phase {}/{} skipped (budget exhausted)",
                        crate::t(),
                        depth + 1,
                        num_depths
                    ),
                );
            }
            break;
        }

        if options.debug.main {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id: starting phase {}/{} n={n} cutoff={c:.1}s remaining={phase_remaining:.1}s",
                    crate::t(),
                    depth + 1,
                    num_depths
                ),
            );
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
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id: phase {}/{} done — score={score:.6}",
                    crate::t(),
                    depth + 1,
                    num_depths
                ),
            );
        }

        current_initial = Some(inc.clone());
        best = Some((inc, score));
    }

    best.ok_or_else(|| anyhow::anyhow!("no phases ran (budget already exhausted)"))
}

/// Sample a random non-forbidden configuration.
///
/// Forbidden clauses are tested against the active projection, not the full
/// draw: a clause naming a parameter that is inactive in this draw must not
/// reject a configuration whose active projection is perfectly legal, since
/// only the active projection is ever evaluated, hashed or sent to the solver.
fn random_config(space: &ParamSpace, rng: &mut impl Rng) -> Config {
    loop {
        let cfg: Config = space
            .params
            .iter()
            .map(|p| (p.name.clone(), p.domain[rng.gen_range(0..p.domain.len())].clone()))
            .collect();
        if !space.is_forbidden(&active_config(&cfg, space)) {
            return cfg;
        }
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

    /// Like `conditional_space`, plus a clause naming `limit`, which is
    /// inactive whenever `mode=fast` — so `{mode=fast, limit=2}` must not
    /// reject a configuration whose active projection is just `{mode=fast}`.
    fn conditional_forbidden_space() -> ParamSpace {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
        writeln!(f, "limit {{1, 2}} [1] | mode in {{slow}}").unwrap();
        writeln!(f, "{{mode=fast, limit=2}}").unwrap();
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
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
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
            let diffs: usize = nb.iter().filter(|(k, v)| config.get(*k) != Some(v)).count();
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
            n_workers: 1,
            perturbation_strength: 4,
            debug: crate::DebugOptions::default(),
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: true,
            tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
        };
        assert!(dominates(1.0, 5, 2.0, 5, &opts)); // strictly better
        assert!(dominates(1.0, 1, 2.0, 10, &opts)); // BasicILS ignores run counts
        assert!(!dominates(2.0, 5, 1.0, 5, &opts)); // worse
        assert!(!dominates(1.0, 5, 1.0, 5, &opts)); // tie — does NOT dominate
    }

    #[test]
    fn dominates_focused() {
        let opts = IlsOptions {
            approach: Approach::Focused,
            n_workers: 1,
            perturbation_strength: 4,
            debug: crate::DebugOptions::default(),
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: true,
            tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
        };
        assert!(dominates(1.0, 10, 2.0, 5, &opts)); // strictly better score, more runs
        assert!(!dominates(1.0, 3, 2.0, 5, &opts)); // better score but fewer runs
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

    /// The softened criterion accepts a worse local optimum only while it stays
    /// inside the band around the *incumbent*, and the band is measured from
    /// the incumbent precisely so that repeated acceptances cannot walk the
    /// home base downhill one margin at a time.
    #[test]
    fn acceptance_tolerance_band_is_anchored_at_the_incumbent() {
        let mut opts = focused_options();
        opts.acceptance_tolerance = 0.05;
        let old = cfg(&[("alpha", "1")]);
        let new = cfg(&[("alpha", "2")]);
        let incumbent_score = 2.0;

        // Worse than the home base but within 5% of the incumbent: accepted.
        let (config, score, took_new) =
            acceptance_criterion(new.clone(), 2.05, 8, old.clone(), 2.0, 8, incumbent_score, &opts);
        assert_eq!(config, new);
        assert!(took_new);
        assert!((score - 2.05).abs() < 1e-9);

        // Outside the band: rejected.
        let (config, score, took_new) =
            acceptance_criterion(new.clone(), 2.2, 8, old.clone(), 2.0, 8, incumbent_score, &opts);
        assert_eq!(config, old);
        assert!(!took_new);
        assert!((score - 2.0).abs() < 1e-9);

        // The band does not drift with the home base: a home base that already
        // sits inside the band cannot pull the bar up behind it.
        let (config, _, took_new) =
            acceptance_criterion(new.clone(), 2.15, 8, old.clone(), 2.1, 8, incumbent_score, &opts);
        assert_eq!(config, old);
        assert!(!took_new);
    }

    /// A zero tolerance has to leave the ParamILS rule untouched, so runs made
    /// before the knob existed stay reproducible.
    #[test]
    fn acceptance_tolerance_zero_is_the_paramils_rule() {
        let opts = focused_options();
        assert_eq!(opts.acceptance_tolerance, 0.0);
        let old = cfg(&[("alpha", "1")]);
        let new = cfg(&[("alpha", "2")]);

        let (config, _, took_new) = acceptance_criterion(new.clone(), 2.000_001, 8, old.clone(), 2.0, 8, 2.0, &opts);
        assert_eq!(config, old, "any worse score is rejected when tolerance is 0");
        assert!(!took_new);
    }

    /// An infinite incumbent score (nothing measured successfully yet) must not
    /// open the band to everything.
    #[test]
    fn acceptance_tolerance_ignores_non_finite_scores() {
        let mut opts = focused_options();
        opts.acceptance_tolerance = 0.05;
        assert!(!accepted_within_tolerance(1.0, f64::INFINITY, &opts));
        assert!(!accepted_within_tolerance(f64::INFINITY, 1.0, &opts));
    }

    /// With a negative objective the band still has to widen upward from the
    /// incumbent, which `incumbent * (1 + tol)` would get backwards.
    #[test]
    fn acceptance_tolerance_handles_negative_scores() {
        let mut opts = focused_options();
        opts.acceptance_tolerance = 0.10;
        // Incumbent -2.0; the band reaches up to -1.8.
        assert!(accepted_within_tolerance(-1.9, -2.0, &opts));
        assert!(!accepted_within_tolerance(-1.7, -2.0, &opts));
    }

    /// A restart from the incumbent is still a perturbation: it must land
    /// inside the space, and it must actually move.
    #[test]
    fn restart_from_incumbent_is_a_stronger_perturbation() {
        let space = simple_space();
        let incumbent = cfg(&[("alpha", "2"), ("beta", "a")]);
        let mut rng = rand::thread_rng();

        let mut moved = 0;
        for _ in 0..50 {
            let restarted = perturbation(incumbent.clone(), 8, &space, &mut rng);
            assert_eq!(restarted.len(), incumbent.len());
            for (name, value) in &restarted {
                let param = space.params.iter().find(|p| &p.name == name).unwrap();
                assert!(param.domain.contains(value), "{name}={value} left its domain");
            }
            if restarted != incumbent {
                moved += 1;
            }
        }
        assert!(moved > 0, "a strength-8 restart never moved in 50 attempts");
    }

    #[test]
    fn acceptance_takes_better_and_ties_but_not_worse() {
        let opts = focused_options();
        let old = cfg(&[("alpha", "1")]);
        let new = cfg(&[("alpha", "2")]);

        // Strictly better: accepted.
        let (config, score, took_new) = acceptance_criterion(new.clone(), 1.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
        assert!(took_new);
        assert_eq!(config, new);
        assert!((score - 1.0).abs() < 1e-9);

        // Tie: accepted, so the home base can cross plateaus.
        let (config, score, took_new) = acceptance_criterion(new.clone(), 2.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
        assert!(took_new);
        assert_eq!(config, new);
        assert!((score - 2.0).abs() < 1e-9);

        // Worse: rejected, home base unchanged.
        let (config, score, took_new) = acceptance_criterion(new.clone(), 3.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
        assert!(!took_new);
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
            n_workers: 1,
            perturbation_strength: 4,
            debug: crate::DebugOptions::default(),
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: false,
            tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Mean,
        };
        assert!((compute_score(&[1.0, 2.0, 3.0], &[0.0; 3], &opts) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn compute_score_median_runtime() {
        let opts = IlsOptions {
            approach: Approach::Basic,
            n_workers: 1,
            perturbation_strength: 4,
            debug: crate::DebugOptions::default(),
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: false,
            tuner_timeout: 60.0,
            run_obj: RunObjective::Runtime,
            overall_obj: OverallObjective::Median,
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
    fn forbidden_clause_on_inactive_parameter_is_not_a_real_constraint() {
        let space = conditional_forbidden_space();
        // `limit` is inactive when mode=fast, so a stale limit=2 sitting in
        // the raw config must not turn into a real constraint.
        let raw = cfg(&[("mode", "fast"), ("limit", "2")]);
        assert!(space.is_forbidden(&raw), "raw config still matches the clause literally");
        assert!(
            !space.is_forbidden(&active_config(&raw, &space)),
            "active projection drops the inactive `limit` entry, so the clause can't match"
        );
    }

    #[test]
    fn neighbourhood_does_not_reject_move_due_to_inactive_forbidden_match() {
        let space = conditional_forbidden_space();
        let config = cfg(&[("mode", "slow"), ("limit", "2")]);
        let n = neighbourhood(&config, &space);
        // From mode=slow,limit=2: mode->fast drops (deactivates) `limit`, so
        // the resulting active projection is just {mode=fast} and the clause
        // {mode=fast, limit=2} must not block the move; limit->1 is unrelated.
        assert_eq!(n.len(), 2);
        assert!(n.iter().any(|c| c.get("mode").map(String::as_str) == Some("fast")));
        assert!(n.iter().any(|c| c.get("limit").map(String::as_str) == Some("1")));
    }

    /// Two children of the same guard, forbidden only in combination with
    /// each other: `a=2,b=2` is fine while both are inactive (mode=fast), but
    /// must be caught the moment a single move flips the shared guard and
    /// activates both at once with their still-stale values.
    fn shared_guard_forbidden_space() -> ParamSpace {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
        writeln!(f, "a {{1, 2}} [1] | mode in {{slow}}").unwrap();
        writeln!(f, "b {{1, 2}} [1] | mode in {{slow}}").unwrap();
        writeln!(f, "{{a=2, b=2}}").unwrap();
        let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
        drop(f);
        space
    }

    #[test]
    fn neighbourhood_catches_forbidden_combo_exposed_by_activating_a_shared_guard() {
        let space = shared_guard_forbidden_space();
        // a=2,b=2 sit dormant while mode=fast; nothing has ever validated
        // that combination against the forbidden clause, because both were
        // inactive whenever the value was assigned (random draw or a prior,
        // independent perturbation step).
        let config = cfg(&[("mode", "fast"), ("a", "2"), ("b", "2")]);
        assert!(!space.is_forbidden(&active_config(&config, &space)), "dormant, so not yet forbidden");

        let n = neighbourhood(&config, &space);
        // The only active param at mode=fast is `mode` itself, so the only
        // neighbour is mode->slow — which activates both a and b at once,
        // exposing the forbidden {a=2,b=2} they were carrying. It must be
        // rejected, leaving no neighbours at all.
        assert!(
            n.iter().all(|c| c.get("mode").map(String::as_str) != Some("slow")),
            "activating the shared guard must not silently surface a forbidden combination: {n:?}"
        );
        assert!(n.is_empty());
    }

    #[test]
    fn random_config_does_not_reject_due_to_inactive_forbidden_match() {
        let space = conditional_forbidden_space();
        let mut rng = rand::thread_rng();
        for _ in 0..200 {
            let cfg = random_config(&space, &mut rng);
            // A forbidden combination naming only an inactive parameter must
            // never make random_config reject a legal active configuration.
            assert!(!space.is_forbidden(&active_config(&cfg, &space)));
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
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
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
        )
        .unwrap();

        assert!(started.elapsed() >= Duration::from_millis(650));
        assert!((score - 0.7).abs() < 1e-9);
    }

    #[test]
    fn evaluation_marks_partial_cache_result_incomplete() {
        let mut cache = Cache::open(":memory:", false).unwrap();
        let paths = vec!["cached.cnf".to_string(), "slow.cnf".to_string()];
        let ids = cache.load_instances(&paths).unwrap();
        let instances = paths.iter().map(|path| (ids[path], path.clone())).collect::<Vec<_>>();
        let space = simple_space();
        let config = cfg(&[("alpha", "1"), ("beta", "a")]);
        let hash = hash_config(&active_config(&config, &space));
        cache.put(hash, ids["cached.cnf"], 0.1, 0.0, "sat", 2.0, None).unwrap();

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
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
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

    #[test]
    fn evaluation_combines_runhash_via_xor() {
        // Both instances get the exact same runhash from this fake wrapper
        // (a constant, not derived from the instance), so a correct XOR
        // combination over the two must cancel to zero -- a cheap way to
        // prove the combiner actually ran over both results rather than,
        // say, just taking the first one.
        let mut cache = Cache::open(":memory:", false).unwrap();
        let paths = vec!["a.cnf".to_string(), "b.cnf".to_string()];
        let ids = cache.load_instances(&paths).unwrap();
        let instances = paths.iter().map(|path| (ids[path], path.clone())).collect::<Vec<_>>();
        let space = simple_space();
        let config = cfg(&[("alpha", "1"), ("beta", "a")]);

        // `printf`, not `echo`: the command is invoked as `{algo} {instance}
        // {cutoff} -k v...`, and unlike `echo`, `printf` with no conversion
        // specifiers in its format ignores the trailing positional args
        // instead of echoing them onto the result line, where they would
        // land inside the runhash field (the last, unbounded split segment)
        // and break its hex parse.
        let scheduler = Scheduler::new(
            1,
            "printf '#%%# RamParIls #%%# sat, 0.1, 0.0, 00000000000000ff\\n'".to_string(),
            2.0,
            crate::DebugOptions::default(),
        );
        let options = IlsOptions {
            approach: Approach::Focused,
            n_workers: 1,
            perturbation_strength: 1,
            restart_probability: 0.0,
            restart_failures: 0,
            restart_target: RestartTarget::Incumbent,
            restart_strength: 8,
            acceptance_tolerance: 0.0,
            random_probes: 0,
            initial_fidelity: 1,
            fidelity_step: 1,
            bound_multiplier: 10.0,
            pruning: false,
            tuner_timeout: 2.0,
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
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();

        assert!(evaluation.complete);
        assert_eq!(evaluation.runhash_n, 2);
        assert_eq!(evaluation.runhash, 0, "XOR of two identical runhashes must cancel to zero");
        assert_eq!(evaluation.runhash_suffix(2), " runhash=0000000000000000 (n=2/2)");
    }
}
