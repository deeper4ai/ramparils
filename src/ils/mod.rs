//! ILS loop: initialization → basic local search → perturbation → acceptance.
//!
//! Mirrors `param_ils_2_3_run.rb`:
//!   iterated_local_search()
//!     init_default() / init_random()
//!     basic_local_search()    — first-improvement, parallel neighbour evaluation
//!     perturbation()          — random walk of strength s
//!     acceptance_criterion()  — accept if the new local optimum dominates the last one
//!
//! # Module layout
//!
//! - `bls` — the parallel first-improvement descent step ([`bls::basic_local_search`])
//!   and the single-config evaluation machinery ([`bls::evaluate_config_outcome`],
//!   [`bls::collect_one`]) it shares with [`run`].
//! - `futell` — checkpoint-based early rejection of BLS neighbours (DONE.md).
//! - `logging` — the `ils: new incumbent`/`ils: new home base` debug lines.
//! - `counters` — atomics behind the end-of-run `ils: summary` line.
//! - `deepening` — the iterative-deepening wrapper around [`run`].
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
use rand::Rng;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use std::time::{Duration, Instant};

use crate::cache::{Cache, hash_config};
use crate::eval::Scheduler;
use crate::params::{Config, ParamSpace, config_to_yaml};
use crate::scenario::{OverallObjective, RunObjective};

mod bls;
mod counters;
mod deepening;
mod futell;
mod logging;

pub use deepening::iterative_deepening_ils;

use bls::{
    ConfigEvaluation, EvalContext, acceptance_criterion, basic_local_search, dominates, evaluate_config,
    evaluate_config_outcome,
};
use logging::{log_home_base, log_incumbent};

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
#[derive(Debug, Clone)]
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
    /// Shuffle `instances` once, deterministically, before dispatch (DONE.md).
    pub instance_shuffle: bool,
    /// Seed for `instance_shuffle`.
    pub instance_shuffle_seed: u64,
    /// Opt-in checkpoint-based early rejection of BLS neighbours (DONE.md).
    pub future_telling: bool,
    /// Checkpoint horizon, as a multiple of `cutoff_time`.
    pub future_telling_checkpoint: f64,
    /// Virtual worker count for the checkpoint simulation. Already resolved
    /// (a `None` in the scenario becomes `n_workers` before this is built).
    pub future_telling_cores: usize,
    /// Relative rejection margin, same shape as `acceptance_tolerance`.
    pub future_telling_tolerance: f64,
    /// Disable cache lookups: every task runs regardless of what's already
    /// cached (writes still happen). See `Scenario::cache_disable`.
    pub cache_disable: bool,
    pub debug: crate::DebugOptions,
}

/// State carried from one ILS round to the next: the incumbent (the best
/// configuration found) and the home base (`last_lm`, where the next
/// perturbation starts from), plus the round-accounting counters behind the
/// end-of-run `ils: summary` line. `n_runs` and the transient `current`/
/// `current_eval` of one round stay as locals in [`run`] — they aren't
/// carried state in the same sense, since every round replaces them outright.
struct RunState {
    incumbent: Config,
    incumbent_score: f64,
    /// Run-local reference checkpoint for future-telling (DONE.md):
    /// reassigned unconditionally everywhere `incumbent_score` is, including
    /// to `None` — a stale `Some` would compare a future challenger against a
    /// reference describing a config that no longer holds that role.
    incumbent_checkpoint: Option<f64>,
    last_lm: Config,
    last_lm_eval: ConfigEvaluation,
    /// Recorded (D9) but not yet consulted by anything — kept for a possible
    /// future acceptance-side use; see DONE.md's open questions. Always
    /// equal to `last_lm_eval.checkpoint` at every point it's read.
    home_base_checkpoint: Option<f64>,
    /// Consecutive rounds whose local optimum failed the acceptance
    /// criterion, which is what `restart_failures` counts.
    rejections: usize,
    /// Counts `ils: new incumbent` lines — replacements of the starting one.
    n_incumbents: usize,
    n_rounds: usize,
    n_searched: usize,
    n_gated: usize,
}

impl RunState {
    /// Seeds both the incumbent and the home base from the same starting
    /// point — the caller's `current`/`current_eval` right before the first
    /// descent runs.
    fn new(start: Config, start_eval: ConfigEvaluation) -> Self {
        Self {
            incumbent: start.clone(),
            incumbent_score: start_eval.score,
            incumbent_checkpoint: start_eval.checkpoint,
            last_lm: start,
            last_lm_eval: start_eval,
            home_base_checkpoint: None,
            rejections: 0,
            n_incumbents: 0,
            n_rounds: 0,
            n_searched: 0,
            n_gated: 0,
        }
    }

    /// Promote `candidate` to incumbent if it (strictly) dominates the
    /// current one; logs `ils: new incumbent` and returns whether it did.
    ///
    /// Refuses an incomplete `eval` outright, before ever comparing scores.
    /// A capped/checkpoint-rejected evaluation's score is a partial mean
    /// over however many instances happened to finish — often optimistic,
    /// not pessimistic, when `future_telling` is what capped it — so it
    /// must never be allowed to win `dominates` against a real, fully
    /// measured incumbent score.
    fn try_promote_incumbent(
        &mut self,
        candidate: &Config,
        eval: &ConfigEvaluation,
        n_runs: usize,
        space: &ParamSpace,
        options: &IlsOptions,
    ) -> Result<bool> {
        if !eval.complete {
            return Ok(false);
        }
        if !dominates(eval.score, n_runs, self.incumbent_score, n_runs, options) {
            return Ok(false);
        }
        self.incumbent = candidate.clone();
        self.incumbent_score = eval.score;
        self.incumbent_checkpoint = eval.checkpoint;
        self.n_incumbents += 1;
        log_incumbent(options.debug.main, &self.incumbent, eval, n_runs, space)?;
        Ok(true)
    }

    /// Replace the home base outright, no acceptance test — used by
    /// `Approach::Random` (which has no home base to keep) and by a restart.
    ///
    /// Refuses an incomplete `eval`: the home base's score/checkpoint feed
    /// straight into every later round's reference and pruning bound, so an
    /// optimistic partial score here would keep corrupting comparisons long
    /// after this one round. An incomplete restart/random candidate is
    /// dropped and the previous home base kept, logged for visibility.
    fn set_home_base(
        &mut self,
        new_home_base: Config,
        eval: ConfigEvaluation,
        n_runs: usize,
        space: &ParamSpace,
        options: &IlsOptions,
    ) {
        if !eval.complete {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: home base candidate incomplete ({}), keeping previous home base",
                    crate::t(),
                    eval.display(n_runs)
                ),
            );
            return;
        }
        let previous = std::mem::replace(&mut self.last_lm, new_home_base);
        log_home_base(options.debug.main, &previous, &self.last_lm, &eval, n_runs, space);
        self.home_base_checkpoint = eval.checkpoint;
        self.last_lm_eval = eval;
    }

    /// Run the acceptance criterion against `new_lm` and update the home
    /// base accordingly, tracking `rejections` for the stagnation restart.
    ///
    /// An incomplete `new_lm_eval` (a gated/capped descent that never got
    /// going, see `gated_start`) is treated as an outright rejection without
    /// ever reaching `acceptance_criterion` — same reasoning as
    /// `set_home_base`: its score is a partial mean, not comparable to the
    /// home base's or incumbent's full-evaluation scores.
    fn accept_or_reject_home_base(
        &mut self,
        new_lm: Config,
        new_lm_eval: ConfigEvaluation,
        n_runs: usize,
        space: &ParamSpace,
        options: &IlsOptions,
    ) {
        if !new_lm_eval.complete {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: home base candidate incomplete ({}), rejected",
                    crate::t(),
                    new_lm_eval.display(n_runs)
                ),
            );
            self.rejections += 1;
            return;
        }
        let previous = self.last_lm.clone();
        let (accepted, accepted_score, took_new) = acceptance_criterion(
            new_lm,
            new_lm_eval.score,
            n_runs,
            self.last_lm.clone(),
            self.last_lm_eval.score,
            n_runs,
            self.incumbent_score,
            options,
        );
        self.last_lm = accepted;
        self.last_lm_eval = if took_new { new_lm_eval } else { self.last_lm_eval };
        debug_assert_eq!(self.last_lm_eval.score, accepted_score);
        self.home_base_checkpoint = self.last_lm_eval.checkpoint;
        self.rejections = if took_new { 0 } else { self.rejections + 1 };
        log_home_base(
            options.debug.main,
            &previous,
            &self.last_lm,
            &self.last_lm_eval,
            n_runs,
            space,
        );
    }

    /// Update the end-of-run round counters for one completed round.
    /// `gated_start` marks a round whose *starting* point was already capped
    /// (so the descent's neighbourhood was invisible from there) — it is
    /// only meaningful when `steps == 0`.
    fn record_round(&mut self, steps: usize, gated_start: bool) {
        self.n_rounds += 1;
        if steps > 0 {
            self.n_searched += 1;
        } else if gated_start {
            self.n_gated += 1;
        }
    }

    /// Whether a restart should fire this round, and why (D-two independent
    /// triggers; either may fire).
    fn restart_reason(&self, options: &IlsOptions, rng: &mut impl Rng) -> Option<RestartReason> {
        if options.restart_failures > 0 && self.rejections >= options.restart_failures {
            Some(RestartReason::Stagnation)
        } else if options.restart_probability > 0.0 && rng.gen_range(0.0..1.0) < options.restart_probability {
            Some(RestartReason::Probability)
        } else {
            None
        }
    }
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
    let scheduler = Scheduler::new(
        options.n_workers,
        algo.to_string(),
        cutoff_time,
        options.cache_disable,
        options.debug,
    );
    let mut rng = rand::thread_rng();
    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache,
        options,
        space,
        cutoff_time,
        deadline,
    };

    // Instance shuffle (DONE.md): a pure reordering of an
    // already-ID-assigned `Vec<(i64, String)>` — `cache.load_instances()` has
    // already run by the time `instances` reaches here, so this never affects
    // which `instance_id` a path resolves to. Decorrelates the fixed
    // evaluation-order prefix (fidelity growth, future-telling's checkpoint
    // simulation) from any difficulty ordering already present in the file.
    let shuffled_instances;
    let instances: &[(i64, String)] = if options.instance_shuffle {
        shuffled_instances = shuffle_instances(instances, options.instance_shuffle_seed);
        crate::debug_line(
            options.debug.main,
            &format!(
                "[{:8.2}s] ils: instance_shuffle applied to {} instances (seed={})",
                crate::t(),
                shuffled_instances.len(),
                options.instance_shuffle_seed
            ),
        );
        &shuffled_instances
    } else {
        instances
    };
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
    let mut current_eval = evaluate_config_outcome(&mut ctx, &current, &instances[..n_runs], None, None)?;

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
            &mut ctx,
            &probe,
            &instances[..n_runs],
            Some(current_eval.score),
            current_eval.checkpoint,
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

    // --- First BLS ---
    // `RunState::new` seeds both the incumbent and the home base from
    // `current`/`current_eval` here, so the home-base diff `set_home_base`
    // logs below is against this exact starting point, same as the
    // incumbent's.
    let mut state = RunState::new(current.clone(), current_eval);
    let (lm, lm_eval, first_steps) = basic_local_search(
        &mut ctx,
        current,
        current_eval,
        &instances[..n_runs],
        state.incumbent_score,
        state.incumbent_checkpoint,
        &mut rng,
    )?;
    state.try_promote_incumbent(&lm, &lm_eval, n_runs, space, options)?;
    state.set_home_base(lm, lm_eval, n_runs, space, options);
    // The first descent counts as a round; it can never be gated (its own
    // starting evaluation above was never capped, having no bound to prune
    // against), which is itself worth seeing in the ratio.
    state.record_round(first_steps, false);

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
            bls::perturbation(state.last_lm.clone(), options.perturbation_strength, space, &mut rng)
        };
        current = perturbed;
        current_eval = evaluate_config_outcome(
            &mut ctx,
            &current,
            &instances[..n_runs],
            Some(state.incumbent_score),
            state.incumbent_checkpoint,
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
            let nb = bls::neighbourhood(&current, space).len();
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: bls neighborhood={nb} instances={n_runs} incumbent={:.6}",
                    crate::t(),
                    state.incumbent_score
                ),
            );
        }
        let (new_lm, new_lm_eval, steps) = basic_local_search(
            &mut ctx,
            current,
            current_eval,
            &instances[..n_runs],
            state.incumbent_score,
            state.incumbent_checkpoint,
            &mut rng,
        )?;
        state.record_round(steps, gated_start);

        // Update incumbent.  `new_lm_eval`, `state.incumbent_score` and
        // `state.last_lm_eval` are all measured on `instances[..n_runs]` here
        // — the fidelity block at the end of the loop re-measures the two
        // retained states together, so the comparisons below never cross
        // fidelities.
        let promoted = state.try_promote_incumbent(&new_lm, &new_lm_eval, n_runs, space, options)?;
        let incumbent_survived = !promoted;

        // Acceptance criterion: keep new local opt only if it dominates the
        // last one.  Skipped entirely under `Approach::Random`, where the next
        // round starts from a fresh random configuration regardless — there is
        // no home base to keep, and a restart would be a no-op.
        if options.approach == Approach::Random {
            state.set_home_base(new_lm, new_lm_eval, n_runs, space, options);
            continue;
        }

        state.accept_or_reject_home_base(new_lm, new_lm_eval, n_runs, space, options);

        // Restart.  The acceptance criterion above can only move the home base
        // to an at-least-as-good local optimum (or, with a tolerance, one
        // close to the incumbent), so on its own it can never move the search
        // uphill: a home base that stops improving stays put for the rest of
        // the budget and every later round perturbs the same point.  Either
        // trigger below breaks that.
        if let Some(reason) = state.restart_reason(options, &mut rng) {
            if Instant::now() >= deadline || crate::interrupted() {
                break;
            }
            let restarted = match options.restart_target {
                RestartTarget::Incumbent => {
                    bls::perturbation(state.incumbent.clone(), options.restart_strength, space, &mut rng)
                }
                RestartTarget::Random => random_config(space, &mut rng),
            };
            // Capped against the incumbent: a restart lands on a configuration
            // that is usually much worse, and there is no reason to pay for a
            // full evaluation of one.  A pruned evaluation yields a score above
            // the cap, which is exactly the "bad home base" the next round is
            // meant to escape from anyway.
            let restarted_eval = evaluate_config_outcome(
                &mut ctx,
                &restarted,
                &instances[..n_runs],
                Some(state.incumbent_score),
                state.incumbent_checkpoint,
            )?;
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] ils: restart: reason={} target={} strength={} score={} instances={n_runs} after {} rejected local optima",
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
                    state.rejections,
                ),
            );
            state.set_home_base(restarted, restarted_eval, n_runs, space, options);
            state.rejections = 0;
        }

        if incumbent_survived && options.approach == Approach::Focused {
            // Incumbent survived — increase fidelity for the next round (up to all instances).
            // This is the bounded increase mechanism: challengers that fail against the
            // current incumbent push it to be evaluated on another fidelity step.
            let next = next_n_runs(n_runs, options.fidelity_step, n_total);
            if next > n_runs {
                let next_evaluation =
                    evaluate_config_outcome(&mut ctx, &state.incumbent, &instances[..next], None, None)?;
                if !(next_evaluation.complete && next_evaluation.score.is_finite()) {
                    crate::debug_line(
                        options.debug.main,
                        &format!(
                            "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete; retaining {n_runs}-run incumbent_score={:.6}",
                            crate::t(),
                            state.incumbent_score
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
                let home_base_is_incumbent = hash_config(&active_config(&state.last_lm, space))
                    == hash_config(&active_config(&state.incumbent, space));
                // Carry the full evaluation (runhash included) rather than
                // just its score, so `last_lm_eval` below keeps that data
                // instead of losing it to a bare `ConfigEvaluation::complete`.
                let home_base_evaluation = if home_base_is_incumbent {
                    next_evaluation
                } else {
                    let home_base_evaluation =
                        evaluate_config_outcome(&mut ctx, &state.last_lm, &instances[..next], None, None)?;
                    if !(home_base_evaluation.complete && home_base_evaluation.score.is_finite()) {
                        crate::debug_line(
                            options.debug.main,
                            &format!(
                                "[{:8.2}s] ils: fidelity increase to {next}/{n_total} incomplete (home base); retaining {n_runs}-run incumbent_score={:.6}",
                                crate::t(),
                                state.incumbent_score
                            ),
                        );
                        break;
                    }
                    home_base_evaluation
                };

                n_runs = next;
                state.incumbent_score = next_evaluation.score;
                state.incumbent_checkpoint = next_evaluation.checkpoint;
                // Both re-measurements above are guarded on `complete`, so the
                // home base's new score is a full one at the new fidelity.
                let home_base_score = home_base_evaluation.score;
                state.last_lm_eval = home_base_evaluation;
                state.home_base_checkpoint = state.last_lm_eval.checkpoint;
                crate::debug_line(
                    options.debug.main,
                    &format!(
                        "[{:8.2}s] ils: n_runs increased to {n_runs}/{n_total} incumbent_score={:.6} home_base_score={home_base_score:.6}",
                        crate::t(),
                        state.incumbent_score
                    ),
                );
            }
        }
    }

    let (evals, capped, futell_rejected) = counters::get();
    crate::debug_line(
        options.debug.main,
        &format!(
            "[{:8.2}s] ils: summary rounds={} searched={} gated={} \
             incumbents={} evals={evals} capped={capped} futell_rejected={futell_rejected}",
            crate::t(),
            state.n_rounds,
            state.n_searched,
            state.n_gated,
            state.n_incumbents,
        ),
    );

    Ok((state.incumbent, state.incumbent_score))
}

fn active_config(config: &Config, space: &ParamSpace) -> Config {
    space
        .active_params(config)
        .into_iter()
        .filter_map(|param| config.get(&param.name).map(|value| (param.name.clone(), value.clone())))
        .collect()
}

/// Shuffle `instances` for `instance_shuffle` (DONE.md), deterministic
/// on `seed`. A pure permutation of an already-ID-assigned slice — never
/// touches which `instance_id` a path resolves to, only the order `run()`
/// operates over afterward.
fn shuffle_instances(instances: &[(i64, String)], seed: u64) -> Vec<(i64, String)> {
    let mut shuffled = instances.to_vec();
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    shuffled.shuffle(&mut rng);
    shuffled
}

fn initial_n_runs(initial_fidelity: usize, n_total: usize) -> usize {
    initial_fidelity.max(1).min(n_total)
}

fn next_n_runs(current: usize, fidelity_step: usize, n_total: usize) -> usize {
    current.saturating_add(fidelity_step.max(1)).min(n_total)
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

#[cfg(test)]
mod tests;
