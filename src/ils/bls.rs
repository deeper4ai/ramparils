//! Basic local search: the parallel first-improvement descent step shared by
//! every `Approach` (Basic, Focused and Random all call [`basic_local_search`]
//! — only fidelity growth and acceptance differ between them, both handled by
//! `super::run`), plus the single-config evaluation machinery it's built on.

use anyhow::Result;
use crossbeam::channel::RecvTimeoutError;
use rand::Rng;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cache::{Cache, hash_config};
use crate::eval::{DispatchEvent, EvalTask, Scheduler, SchedulerEvent, TaskResult};
use crate::params::{Config, ParamSpace};
use crate::scenario::RunObjective;

use super::futell::{CheckpointTracker, future_telling_active, future_telling_rejects};
use super::logging::format_argument_changes;
use super::{Approach, IlsOptions, active_config, counters};

/// Bundles the handles and settings shared by every evaluation call within
/// one `run()` — grouped so call sites don't repeat six parameters each.
/// Built once in `run()` and threaded through by `&mut` reference; `cache`
/// is the only field anything actually mutates.
pub(super) struct EvalContext<'a> {
    pub(super) scheduler: &'a Scheduler,
    pub(super) cache: &'a mut Cache,
    pub(super) options: &'a IlsOptions,
    pub(super) space: &'a ParamSpace,
    pub(super) cutoff_time: f64,
    pub(super) deadline: Instant,
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
pub(super) fn dominates(a_score: f64, a_runs: usize, b_score: f64, b_runs: usize, options: &IlsOptions) -> bool {
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
pub(super) fn weakly_dominates(a_score: f64, a_runs: usize, b_score: f64, b_runs: usize, options: &IlsOptions) -> bool {
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
pub(super) fn acceptance_criterion(
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
pub(super) fn accepted_within_tolerance(new_score: f64, incumbent_score: f64, options: &IlsOptions) -> bool {
    if options.acceptance_tolerance <= 0.0 {
        return false;
    }
    if !new_score.is_finite() || !incumbent_score.is_finite() {
        return false;
    }
    new_score <= incumbent_score + options.acceptance_tolerance * incumbent_score.abs()
}

/// All one-parameter-away neighbours of `config` within `space`.
/// Only iterates over active params; skips forbidden combinations, tested
/// against each candidate's active projection (see `random_config`).
pub(super) fn neighbourhood(config: &Config, space: &ParamSpace) -> Vec<Config> {
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
pub(super) fn perturbation(config: Config, strength: usize, space: &ParamSpace, rng: &mut impl Rng) -> Config {
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

/// Persist one solver result to the cache, if it's eligible to be.
/// `UNKNOWN` is excluded deliberately: a run that concluded nothing should
/// not be served back to a later config as if it had.
fn cache_result(cache: &mut Cache, r: &TaskResult) -> Result<()> {
    if r.cacheable && r.status != "UNKNOWN" {
        cache.put(
            r.hash,
            r.instance_id,
            r.runtime,
            r.quality,
            &r.status,
            r.cutoff,
            r.runhash,
        )?;
    }
    Ok(())
}

/// Drains any events already queued after a `scheduler.reset()`, writing
/// back whatever `Completed` results were in flight before cancellation.
/// `Dispatched` events (FUTURETELL.md D11) carry nothing to cache and are
/// simply discarded here -- the round is over, nothing more consults their
/// tracker.
fn drain_and_cache_writeback(scheduler: &Scheduler, cache: &mut Cache) -> Result<()> {
    while let Ok(event) = scheduler.events().try_recv() {
        let SchedulerEvent::Completed(r) = event else { continue };
        cache_result(cache, &r)?;
    }
    Ok(())
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
pub(super) struct ConfigEvaluation {
    pub(super) score: f64,
    pub(super) complete: bool,
    pub(super) n_done: usize,
    /// XOR of the `runhash` of every contributing instance that terminated
    /// (see `eval::TaskResult::runhash`). Meaningless when `runhash_n == 0`.
    pub(super) runhash: u64,
    /// How many instances contributed to `runhash` — always `<= n_done`,
    /// since a timeout/error/unknown result carries no runhash. Comparing two
    /// evaluations' `runhash` is only informative when their `runhash_n`
    /// agree; a partial batch is measured over a different set of instances.
    pub(super) runhash_n: usize,
    /// This evaluation's own `CheckpointTracker::score()` at whatever point
    /// it stopped being updated (FUTURETELL.md D7) — `Some` only when the
    /// checkpoint matured while genuinely still in flight; `None` when
    /// future-telling was inactive (D3), never matured, or this evaluation
    /// carries no checkpoint data at all (e.g. a random probe). Read by
    /// `run()` at every incumbent/home-base update site to keep
    /// `incumbent_checkpoint`/`home_base_checkpoint` current.
    pub(super) checkpoint: Option<f64>,
}

impl ConfigEvaluation {
    /// A complete evaluation with no runhash or checkpoint data (e.g. a
    /// random probe, whose caller only kept the scalar score).
    pub(super) fn complete(score: f64, n_done: usize) -> Self {
        Self {
            score,
            complete: true,
            n_done,
            runhash: 0,
            runhash_n: 0,
            checkpoint: None,
        }
    }

    /// `2.698475` when complete, `>2.698475 (312/473)` when capped.
    pub(super) fn display(&self, n_instances: usize) -> String {
        if self.complete {
            format!("{:.6}", self.score)
        } else {
            format!(">{:.6} ({}/{})", self.score, self.n_done, n_instances)
        }
    }

    /// `runhash=<hex> (n=<runhash_n>/<n_instances>)`, or empty when nothing
    /// contributed — appended to a log line beside `display()`.
    pub(super) fn runhash_suffix(&self, n_instances: usize) -> String {
        if self.runhash_n == 0 {
            String::new()
        } else {
            format!(" runhash={:016x} (n={}/{n_instances})", self.runhash, self.runhash_n)
        }
    }
}

/// Evaluate `config` on all instances in parallel.  Returns the scalar score.
///
/// Cache hits are served immediately; misses are dispatched to worker threads.
/// Adaptive capping prunes as soon as the running sum exceeds the budget
/// `bound_multiplier × incumbent_score × n_instances`.
pub(super) fn evaluate_config(
    ctx: &mut EvalContext,
    config: &Config,
    instances: &[(i64, String)],
    incumbent_score: Option<f64>,
    incumbent_checkpoint: Option<f64>,
) -> Result<f64> {
    Ok(evaluate_config_outcome(ctx, config, instances, incumbent_score, incumbent_checkpoint)?.score)
}

pub(super) fn evaluate_config_outcome(
    ctx: &mut EvalContext,
    config: &Config,
    instances: &[(i64, String)],
    incumbent_score: Option<f64>,
    incumbent_checkpoint: Option<f64>,
) -> Result<ConfigEvaluation> {
    if instances.is_empty() {
        return Ok(ConfigEvaluation::complete(0.0, 0));
    }

    let eval_config = active_config(config, ctx.space);
    let hash = hash_config(&eval_config);
    let batch_id = ctx.scheduler.submit(
        vec![EvalTask {
            neighbor_id: 0,
            config: eval_config,
            hash,
            instances: Arc::new(instances.to_vec()),
        }],
        ctx.cache,
    )?;

    collect_one(ctx, batch_id, instances.len(), 0, incumbent_score, incumbent_checkpoint)
}

/// `neighbourhood(config, space)`, reordered for random first-improvement:
/// which neighbour wins a tie in completion order is otherwise just an
/// artefact of how the parameter file happens to list values.
fn shuffled_neighbourhood(config: &Config, space: &ParamSpace, rng: &mut impl Rng) -> Vec<Config> {
    let mut neighbors = neighbourhood(config, space);
    for i in (1..neighbors.len()).rev() {
        let j = rng.gen_range(0..=i);
        neighbors.swap(i, j);
    }
    neighbors
}

/// Submit every neighbour of one BLS round as a single batch, all evaluated
/// against the same `instances` slice.
fn submit_neighbours(ctx: &mut EvalContext, neighbors: &[Config], instances: &[(i64, String)]) -> Result<u64> {
    let shared_instances = Arc::new(instances.to_vec());
    let tasks: Vec<EvalTask> = neighbors
        .iter()
        .enumerate()
        .map(|(i, cfg)| {
            let eval_config = active_config(cfg, ctx.space);
            let hash = hash_config(&eval_config);
            EvalTask {
                neighbor_id: i,
                config: eval_config,
                hash,
                instances: Arc::clone(&shared_instances),
            }
        })
        .collect();
    ctx.scheduler.submit(tasks, ctx.cache)
}

fn log_local_optimum(debug: bool, eval: &ConfigEvaluation, n_instances: usize) {
    crate::debug_line(
        debug,
        &format!(
            "[{:8.2}s] ils: bls local optimum score={}",
            crate::t(),
            eval.display(n_instances)
        ),
    );
}

/// Per-neighbour bookkeeping for one BLS round: every neighbour in the
/// current batch is tracked here until it's `done` (fully completed, capped,
/// or checkpoint-rejected).
struct NeighbourRound {
    n: usize,
    runtimes: Vec<Vec<f64>>,
    qualities: Vec<Vec<f64>>,
    partial: Vec<f64>,
    runhashes: Vec<u64>,
    runhash_ns: Vec<usize>,
    done: Vec<bool>,
    n_done: usize,
    /// Logged once per neighbour, the moment its own checkpoint first
    /// matures — independent of whether it ends up rejected, so the
    /// checkpoint value itself is visible for every neighbour that reaches
    /// one, not just the rejected ones. Temporary, kept on purpose while
    /// future-telling is still being validated against real data.
    checkpoint_logged: Vec<bool>,
    /// Future-telling (D3): one tracker per neighbour, or `None` when
    /// inactive — gated by D3, so no trackers are built at all, not just
    /// unused.
    trackers: Option<Vec<CheckpointTracker>>,
}

impl NeighbourRound {
    fn new(ctx: &mut EvalContext, n: usize, n_instances: usize) -> Self {
        let future_telling_time = ctx.cutoff_time * ctx.options.future_telling_checkpoint;
        let trackers = future_telling_active(ctx.options, n_instances).then(|| {
            (0..n)
                .map(|_| {
                    CheckpointTracker::new(
                        ctx.options.future_telling_cores,
                        future_telling_time,
                        ctx.cutoff_time,
                        n_instances,
                    )
                })
                .collect()
        });
        Self {
            n,
            runtimes: vec![vec![]; n],
            qualities: vec![vec![]; n],
            partial: vec![0.0; n],
            runhashes: vec![0; n],
            runhash_ns: vec![0; n],
            done: vec![false; n],
            n_done: 0,
            checkpoint_logged: vec![false; n],
            trackers,
        }
    }

    /// Run this round's collection loop to completion: either a neighbour is
    /// accepted (returns its config and evaluation), or every neighbour
    /// reaches a verdict without one dominating `current_eval` (returns
    /// `None`). Leaves the scheduler drained and caught up on cache
    /// write-back either way.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        ctx: &mut EvalContext,
        batch_id: u64,
        neighbors: &[Config],
        current: &Config,
        current_eval: ConfigEvaluation,
        incumbent_score: f64,
        incumbent_checkpoint: Option<f64>,
        n_instances: usize,
    ) -> Result<Option<(Config, ConfigEvaluation)>> {
        loop {
            if self.n_done >= self.n || crate::interrupted() {
                self.finish(ctx)?;
                return Ok(None);
            }
            let remaining = ctx.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.finish(ctx)?;
                return Ok(None);
            }

            let event = match ctx
                .scheduler
                .events()
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    // No new event, but real time has still passed -- give
                    // every tracker a chance to notice a still-running
                    // instance has aged past its checkpoint horizon on its
                    // own (FUTURETELL.md D11's `poll`).
                    self.poll_trackers();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.finish(ctx)?;
                    return Ok(None);
                }
            };

            let result = match event {
                SchedulerEvent::Dispatched(d) => {
                    self.record_dispatch(batch_id, d);
                    continue;
                }
                SchedulerEvent::Completed(r) => r,
            };

            cache_result(ctx.cache, &result)?;
            if result.batch_id != batch_id {
                continue;
            }
            let nid = result.neighbor_id;
            // Guard against stale results from a previous reset (shouldn't
            // normally happen, but the window between reset() and drain is tiny).
            if nid >= self.n || self.done[nid] {
                continue;
            }

            self.accumulate(ctx.options, nid, &result);

            if self.check_capping(ctx, batch_id, nid, incumbent_score, n_instances) {
                continue;
            }

            if self.runtimes[nid].len() == n_instances {
                if let Some(eval) = self.complete(ctx, nid, current, &neighbors[nid], current_eval, n_instances)? {
                    return Ok(Some((neighbors[nid].clone(), eval)));
                }
            } else {
                self.check_future_telling(ctx, batch_id, nid, incumbent_checkpoint, n_instances);
            }
        }
    }

    fn record_dispatch(&mut self, batch_id: u64, d: DispatchEvent) {
        if d.batch_id != batch_id {
            return;
        }
        let nid = d.neighbor_id;
        if nid < self.n && !self.done[nid] {
            if let Some(trackers) = self.trackers.as_mut() {
                trackers[nid].record_dispatch(d.instance_id, d.dispatch_time);
            }
        }
    }

    fn poll_trackers(&mut self) {
        if let Some(trackers) = self.trackers.as_mut() {
            let now = crate::t();
            for tracker in trackers.iter_mut() {
                tracker.poll(now);
            }
        }
    }

    fn accumulate(&mut self, options: &IlsOptions, nid: usize, result: &TaskResult) {
        self.runtimes[nid].push(result.runtime);
        self.qualities[nid].push(result.quality);
        if let Some(h) = result.runhash {
            self.runhashes[nid] ^= h;
            self.runhash_ns[nid] += 1;
        }
        let val = match options.run_obj {
            RunObjective::Runtime => result.runtime,
            RunObjective::Quality => result.quality,
        };
        self.partial[nid] += val;
        if let Some(trackers) = self.trackers.as_mut() {
            trackers[nid].record(result.instance_id, result.runtime);
        }
    }

    fn tracker_score(&self, nid: usize) -> Option<f64> {
        self.trackers.as_ref().and_then(|t| t[nid].score())
    }

    fn mark_done(&mut self, nid: usize) {
        self.done[nid] = true;
        self.n_done += 1;
    }

    /// Adaptive capping: prune this neighbour once it has spent the whole
    /// budget that beating the incumbent allows. Costs never go down, so
    /// passing the budget *proves* the final mean exceeds the bound — this
    /// is a decision, not a guess, and it is the earliest point at which the
    /// proof exists. Returns whether it fired.
    fn check_capping(
        &mut self,
        ctx: &mut EvalContext,
        batch_id: u64,
        nid: usize,
        incumbent_score: f64,
        n_instances: usize,
    ) -> bool {
        if !ctx.options.pruning {
            return false;
        }
        let budget = ctx.options.bound_multiplier * incumbent_score * n_instances as f64;
        if self.partial[nid] <= budget {
            return false;
        }
        crate::debug_line(
            ctx.options.debug.main,
            &format!(
                "[{:8.2}s] ils: capped neighbor={nid} spent={:.6} budget={budget:.6} after {}/{n_instances}",
                crate::t(),
                self.partial[nid],
                self.runtimes[nid].len(),
            ),
        );
        self.mark_done(nid);
        counters::eval(true);
        // Stop this neighbour's own dispatch, not just the ILS's bookkeeping
        // about it — see `cancel_neighbor`'s own doc comment for why this
        // used to be a no-op in practice.
        ctx.scheduler.cancel_neighbor(batch_id, nid);
        true
    }

    /// This neighbour has just reached full completion. Returns its
    /// evaluation if it dominates `current_eval` and should be accepted
    /// (ending the round, having already reset the scheduler and drained
    /// its leftover events); `None` if it's merely recorded as done and the
    /// round continues.
    fn complete(
        &mut self,
        ctx: &mut EvalContext,
        nid: usize,
        current: &Config,
        neighbor: &Config,
        current_eval: ConfigEvaluation,
        n_instances: usize,
    ) -> Result<Option<ConfigEvaluation>> {
        self.mark_done(nid);
        counters::eval(false);
        let score = compute_score(&self.runtimes[nid], &self.qualities[nid], ctx.options);
        let checkpoint = self.tracker_score(nid);
        if !dominates(score, n_instances, current_eval.score, n_instances, ctx.options) {
            return Ok(None);
        }

        // Accept — stop evaluating the rest.
        ctx.scheduler.reset();
        drain_and_cache_writeback(ctx.scheduler, ctx.cache)?;
        crate::debug_line(
            ctx.options.debug.main,
            &format!(
                "[{:8.2}s] ils: bls improvement neighbor={nid} score={score:.6} (was {})",
                crate::t(),
                current_eval.display(n_instances)
            ),
        );
        crate::debug_line(
            ctx.options.debug.main,
            &format!(
                "[{:8.2}s] ils: bls arguments: {}",
                crate::t(),
                format_argument_changes(current, neighbor, ctx.space)
            ),
        );
        Ok(Some(ConfigEvaluation {
            score,
            complete: true,
            n_done: n_instances,
            runhash: self.runhashes[nid],
            runhash_n: self.runhash_ns[nid],
            checkpoint,
        }))
    }

    /// Consult this neighbour's checkpoint tracker while it's still running:
    /// log once when it first matures, then reject it if it's significantly
    /// worse than the incumbent's own checkpoint (D8). Full completion
    /// (`complete`, above) always wins over a checkpoint verdict — this is
    /// only reached while the neighbour is still incomplete (FUTURETELL.md
    /// "Where this lives").
    fn check_future_telling(
        &mut self,
        ctx: &mut EvalContext,
        batch_id: u64,
        nid: usize,
        incumbent_checkpoint: Option<f64>,
        n_instances: usize,
    ) {
        let Some(chal_ckpt) = self.tracker_score(nid) else {
            return;
        };

        if !self.checkpoint_logged[nid] {
            self.checkpoint_logged[nid] = true;
            crate::debug_line(
                ctx.options.debug.main,
                &format!(
                    "[{:8.2}s] ils: future-telling-checkpoint neighbor={nid} solved={} ref={} after {}/{n_instances}",
                    crate::t(),
                    n_instances - chal_ckpt as usize,
                    incumbent_checkpoint.map_or_else(|| "none".to_string(), |v| (n_instances - v as usize).to_string()),
                    self.runtimes[nid].len(),
                ),
            );
        }

        let Some(inc_ckpt) = incumbent_checkpoint else { return };
        if !future_telling_rejects(chal_ckpt, inc_ckpt, ctx.options) {
            return;
        }
        crate::debug_line(
            ctx.options.debug.main,
            &format!(
                "[{:8.2}s] ils: future-telling-rejected neighbor={nid} solved={} ref={} after {}/{n_instances}",
                crate::t(),
                n_instances - chal_ckpt as usize,
                n_instances - inc_ckpt as usize,
                self.runtimes[nid].len(),
            ),
        );
        self.mark_done(nid);
        counters::eval(true);
        counters::future_telling_rejected();
        ctx.scheduler.cancel_neighbor(batch_id, nid);
    }

    /// No neighbour in this round improved on `current` — reset the
    /// scheduler and drain any leftover events for cache write-back.
    fn finish(&self, ctx: &mut EvalContext) -> Result<()> {
        ctx.scheduler.reset();
        drain_and_cache_writeback(ctx.scheduler, ctx.cache)
    }
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
///
/// `instances` is the exact slice to evaluate every neighbour against —
/// already fidelity-sliced by the caller (`&full_instances[..n_runs]`).
pub(super) fn basic_local_search(
    ctx: &mut EvalContext,
    start: Config,
    start_eval: ConfigEvaluation,
    instances: &[(i64, String)],
    incumbent_score: f64,
    incumbent_checkpoint: Option<f64>,
    rng: &mut impl Rng,
) -> Result<(Config, ConfigEvaluation, usize)> {
    let n_instances = instances.len();
    let mut current = start;
    // Only ever replaced by a neighbour that ran to completion, so after the
    // first accepted step this is a real score even if `start_eval` was capped.
    let mut current_eval = start_eval;
    let mut steps = 0usize;

    while Instant::now() < ctx.deadline && !crate::interrupted() {
        let neighbors = shuffled_neighbourhood(&current, ctx.space, rng);
        if neighbors.is_empty() {
            break;
        }

        let batch_id = submit_neighbours(ctx, &neighbors, instances)?;
        let mut round = NeighbourRound::new(ctx, neighbors.len(), n_instances);

        match round.run(
            ctx,
            batch_id,
            &neighbors,
            &current,
            current_eval,
            incumbent_score,
            incumbent_checkpoint,
            n_instances,
        )? {
            Some((next, next_eval)) => {
                current = next;
                current_eval = next_eval;
                steps += 1;
            }
            None => {
                log_local_optimum(ctx.options.debug.main, &current_eval, n_instances);
                break;
            }
        }
    }

    Ok((current, current_eval, steps))
}

/// Collect exactly `n_instances` results for one config (neighbor_id =
/// `expected_nid`). Used by `evaluate_config_outcome` for single-config
/// evaluation.
pub(super) fn collect_one(
    ctx: &mut EvalContext,
    batch_id: u64,
    n_instances: usize,
    expected_nid: usize,
    incumbent_score: Option<f64>,
    incumbent_checkpoint: Option<f64>,
) -> Result<ConfigEvaluation> {
    let mut collector = SingleConfigCollector::new(ctx, n_instances);
    collector.run(ctx, batch_id, expected_nid, incumbent_score, incumbent_checkpoint)?;
    Ok(collector.finish(ctx))
}

/// Bookkeeping for [`collect_one`] — the same shape as [`NeighbourRound`]
/// but for a single config, so most methods mirror it 1:1 with the
/// per-neighbour `Vec` indexing dropped.
struct SingleConfigCollector {
    n_instances: usize,
    runtimes: Vec<f64>,
    qualities: Vec<f64>,
    partial_sum: f64,
    runhash: u64,
    runhash_n: usize,
    /// Logged once, the moment this evaluation's own checkpoint first
    /// matures — independent of whether a reference exists to compare it
    /// against, so the checkpoint value itself is visible even when nothing
    /// gets rejected. Temporary, kept on purpose while future-telling is
    /// still being validated against real data.
    checkpoint_logged: bool,
    /// Future-telling (D3): only built when the mechanism can possibly
    /// mature before real completion anyway.
    tracker: Option<CheckpointTracker>,
}

impl SingleConfigCollector {
    fn new(ctx: &mut EvalContext, n_instances: usize) -> Self {
        let future_telling_time = ctx.cutoff_time * ctx.options.future_telling_checkpoint;
        let tracker = future_telling_active(ctx.options, n_instances).then(|| {
            CheckpointTracker::new(
                ctx.options.future_telling_cores,
                future_telling_time,
                ctx.cutoff_time,
                n_instances,
            )
        });
        Self {
            n_instances,
            runtimes: Vec::with_capacity(n_instances),
            qualities: Vec::with_capacity(n_instances),
            partial_sum: 0.0,
            runhash: 0,
            runhash_n: 0,
            checkpoint_logged: false,
            tracker,
        }
    }

    fn complete(&self) -> bool {
        self.runtimes.len() >= self.n_instances
    }

    /// Run the collection loop until `n_instances` real results arrive, the
    /// evaluation gets capped, its checkpoint gets rejected, or the
    /// deadline passes.
    fn run(
        &mut self,
        ctx: &mut EvalContext,
        batch_id: u64,
        expected_nid: usize,
        incumbent_score: Option<f64>,
        incumbent_checkpoint: Option<f64>,
    ) -> Result<()> {
        while !self.complete() {
            let remaining = ctx.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || crate::interrupted() {
                return Ok(());
            }

            let event = match ctx
                .scheduler
                .events()
                .recv_timeout(remaining.min(Duration::from_millis(500)))
            {
                Ok(e) => e,
                Err(RecvTimeoutError::Timeout) => {
                    self.poll_tracker();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            };

            let result = match event {
                SchedulerEvent::Dispatched(d) => {
                    self.record_dispatch(batch_id, expected_nid, d);
                    continue;
                }
                SchedulerEvent::Completed(r) => r,
            };

            cache_result(ctx.cache, &result)?;
            if result.batch_id != batch_id || result.neighbor_id != expected_nid {
                continue;
            }

            self.accumulate(ctx.options, &result);

            if self.check_capping(ctx, incumbent_score)? {
                return Ok(());
            }
            // Full completion always wins over a checkpoint verdict
            // (FUTURETELL.md "Where this lives") — only consult the tracker
            // while genuinely still incomplete, never after, even in the
            // edge case where the same arriving result both completes this
            // evaluation and matures its checkpoint.
            if !self.complete() && self.check_future_telling(ctx, incumbent_checkpoint)? {
                return Ok(());
            }
        }
        Ok(())
    }

    fn poll_tracker(&mut self) {
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.poll(crate::t());
        }
    }

    fn record_dispatch(&mut self, batch_id: u64, expected_nid: usize, d: DispatchEvent) {
        if d.batch_id != batch_id || d.neighbor_id != expected_nid {
            return;
        }
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.record_dispatch(d.instance_id, d.dispatch_time);
        }
    }

    fn accumulate(&mut self, options: &IlsOptions, result: &TaskResult) {
        let val = match options.run_obj {
            RunObjective::Runtime => result.runtime,
            RunObjective::Quality => result.quality,
        };
        self.partial_sum += val;
        self.runtimes.push(result.runtime);
        self.qualities.push(result.quality);
        if let Some(h) = result.runhash {
            self.runhash ^= h;
            self.runhash_n += 1;
        }
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.record(result.instance_id, result.runtime);
        }
    }

    /// Same budget test as `NeighbourRound::check_capping`; see there.
    /// Returns whether it fired, in which case the scheduler is already
    /// reset and drained.
    fn check_capping(&mut self, ctx: &mut EvalContext, incumbent_score: Option<f64>) -> Result<bool> {
        let Some(inc) = incumbent_score.filter(|_| ctx.options.pruning) else {
            return Ok(false);
        };
        if self.partial_sum <= ctx.options.bound_multiplier * inc * self.n_instances as f64 {
            return Ok(false);
        }
        ctx.scheduler.reset();
        drain_and_cache_writeback(ctx.scheduler, ctx.cache)?;
        Ok(true)
    }

    /// Consult the checkpoint tracker while still incomplete: log once when
    /// it first matures, then reject (reset + drain) it if it's
    /// significantly worse than the incumbent's own checkpoint (D8).
    /// Returns whether it fired.
    fn check_future_telling(&mut self, ctx: &mut EvalContext, incumbent_checkpoint: Option<f64>) -> Result<bool> {
        let Some(chal_ckpt) = self.tracker.as_ref().and_then(CheckpointTracker::score) else {
            return Ok(false);
        };
        let n_instances = self.n_instances;

        if !self.checkpoint_logged {
            self.checkpoint_logged = true;
            crate::debug_line(
                ctx.options.debug.main,
                &format!(
                    "[{:8.2}s] ils: future-telling-checkpoint config solved={} ref={} after {}/{n_instances}",
                    crate::t(),
                    n_instances - chal_ckpt as usize,
                    incumbent_checkpoint.map_or_else(|| "none".to_string(), |v| (n_instances - v as usize).to_string()),
                    self.runtimes.len(),
                ),
            );
        }

        let Some(inc_ckpt) = incumbent_checkpoint else {
            return Ok(false);
        };
        if !future_telling_rejects(chal_ckpt, inc_ckpt, ctx.options) {
            return Ok(false);
        }
        crate::debug_line(
            ctx.options.debug.main,
            &format!(
                "[{:8.2}s] ils: future-telling-rejected config solved={} ref={} after {}/{n_instances}",
                crate::t(),
                n_instances - chal_ckpt as usize,
                n_instances - inc_ckpt as usize,
                self.runtimes.len(),
            ),
        );
        counters::future_telling_rejected();
        ctx.scheduler.reset();
        drain_and_cache_writeback(ctx.scheduler, ctx.cache)?;
        Ok(true)
    }

    fn finish(self, ctx: &mut EvalContext) -> ConfigEvaluation {
        let complete = self.complete();
        let score = if self.runtimes.is_empty() {
            f64::INFINITY
        } else {
            compute_score(&self.runtimes, &self.qualities, ctx.options)
        };
        counters::eval(!complete);
        let checkpoint = self.tracker.as_ref().and_then(CheckpointTracker::score);
        ConfigEvaluation {
            score,
            complete,
            n_done: self.runtimes.len(),
            runhash: self.runhash,
            runhash_n: self.runhash_n,
            checkpoint,
        }
    }
}

/// Compute a scalar score from per-instance results.
pub(super) fn compute_score(runtimes: &[f64], qualities: &[f64], options: &IlsOptions) -> f64 {
    let values: &[f64] = match options.run_obj {
        RunObjective::Runtime => runtimes,
        RunObjective::Quality => qualities,
    };
    match options.overall_obj {
        crate::scenario::OverallObjective::Mean => values.iter().sum::<f64>() / values.len() as f64,
        crate::scenario::OverallObjective::Median => {
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
