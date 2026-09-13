//! Future-telling: checkpoint-based early rejection of BLS neighbours
//! (FUTURETELL.md).

use std::collections::HashMap;

use super::IlsOptions;

/// Replays `n_total` instances across `n_virtual` identical virtual workers.
/// Two feeds, both routing into the same `virtual_busy` buckets
/// (FUTURETELL.md D11):
///
/// - **Dispatch-tracked** (`record_dispatch` then `record`, correlated by
///   `instance_id`): for a real solver invocation, busy-time is claimed the
///   instant a worker *starts* the instance, not when its result arrives —
///   so a still-running instance can be known to be busy past the horizon
///   without waiting for its result to physically arrive. Superseded from
///   the pure arrival-based design (kept here as the record of why it
///   changed): measured in a production run, accounting for busy-time only
///   retrospectively from completions made a
///   64%-timeout config's checkpoint take ~10.6s of real wall clock against
///   a nominal 5s horizon, needing ~164 arrivals to fill all 64 buckets
///   instead of one wave's worth.
/// - **Fallback** (`record` alone, no matching `record_dispatch`): a cache
///   hit, or any dispatch this tracker never saw (e.g. more concurrently
///   in-flight real dispatches than `n_virtual` buckets — not yet designed
///   for, see FUTURETELL.md D11's "not yet decided" note). Assigns to
///   whichever bucket is currently least busy and chains onto its existing
///   relative-time total — exactly the old, purely arrival-order-driven
///   behaviour, so a tracker fed only through `record` (never
///   `record_dispatch`) is unchanged from before.
///
/// **Why dispatch times are relative to this neighbour's own first dispatch,
/// never to the round's `submit()` call**: `basic_local_search` submits every
/// neighbour's `WorkBatch` in one call, but the shared worker pool drains
/// them one neighbour at a time (confirmed empirically, `eval.rs`'s
/// `submit()`+worker-loop mechanics) — a neighbour queued behind others sees
/// its first real dispatch well after `submit()` returns, through no fault
/// of its own. Anchoring to `submit()` would count that queue-wait against
/// its checkpoint budget, exactly the cross-neighbour contamination D1
/// rejected reading real wall-clock time over in the first place. Anchoring
/// to each neighbour's *own* first `Dispatched` event avoids that — at the
/// known cost that this neighbour's own first few dispatches still trickle
/// in one at a time (as each real worker frees from the *previous*
/// neighbour's tail) rather than bursting in simultaneously, so the clean
/// bound `collect_one` gets (one dedicated config, true t≈0 for every
/// worker) doesn't fully carry over here — a smaller, accepted version of
/// the same effect, not a bug.
pub(super) struct CheckpointTracker {
    pub(super) n_total: usize,
    pub(super) checkpoint_time: f64,
    pub(super) cutoff_time: f64,
    /// This neighbour's own relative-time origin: `crate::t()` at its first
    /// `record_dispatch` call. Never set from a cache hit or any other
    /// `record`-only arrival. `None` until that first dispatch.
    pub(super) t0: Option<f64>,
    /// Per virtual bucket: busy-until, in this neighbour's own relative
    /// time (`0.0` initially). Meaningful only for a bucket with no entry
    /// in `pending` below.
    pub(super) virtual_busy: Vec<f64>,
    /// Buckets currently occupied by a dispatched-but-not-yet-completed
    /// instance: bucket -> (instance_id, its own relative dispatch time).
    /// Absence of a bucket here means it is free for the next assignment.
    pub(super) pending: HashMap<usize, (i64, f64)>,
    /// instance_id -> bucket, so the matching `record` can settle the same
    /// bucket its `record_dispatch` claimed instead of picking a fresh one.
    pub(super) assigned: HashMap<i64, usize>,
    /// Count of results virtually finished by `checkpoint_time` with
    /// `runtime < cutoff_time` — an exact "solved" count for any wrapper
    /// following the PAR1 contract (`docs/reference/protocol.md`), not an
    /// approximation of one.
    pub(super) solved_count: usize,
    /// Sum of `runtime` over every result virtually finished by
    /// `checkpoint_time`, solved or not — the numerator of [`par1`](Self::par1),
    /// logged alongside `score()` purely for comparison (D6 still decides
    /// on `solved_count`). Under the PAR1 contract a non-solved result's own
    /// `runtime` already reports `cutoff_time`, so summing it raw (not
    /// re-clamping) is correct.
    pub(super) time_sum: f64,
    /// How many results contributed to `time_sum` — always `<= n_seen`,
    /// counted separately from `solved_count` because it includes
    /// non-solved results that still finished within the horizon.
    pub(super) n_counted: usize,
    /// How many real results this tracker has seen so far.
    pub(super) n_seen: usize,
    /// True once every virtual bucket is known to be busy past
    /// `checkpoint_time` — either a confirmed `virtual_busy` value past it,
    /// or a still-pending dispatch where relative "now" has already passed
    /// it (so its eventual finish, whatever it turns out to be, must be
    /// later still). Deliberately never set once `n_seen == n_total` — see
    /// `score()`.
    pub(super) ready: bool,
}

impl CheckpointTracker {
    pub(super) fn new(n_virtual: usize, checkpoint_time: f64, cutoff_time: f64, n_total: usize) -> Self {
        Self {
            n_total,
            checkpoint_time,
            cutoff_time,
            t0: None,
            virtual_busy: vec![0.0; n_virtual.max(1)],
            pending: HashMap::new(),
            assigned: HashMap::new(),
            solved_count: 0,
            time_sum: 0.0,
            n_counted: 0,
            n_seen: 0,
            ready: false,
        }
    }

    fn least_busy_free_bucket(&self) -> Option<usize> {
        self.virtual_busy
            .iter()
            .enumerate()
            .filter(|(w, _)| !self.pending.contains_key(w))
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(w, _)| w)
    }

    /// A real worker has just claimed `instance_id` (`eval.rs`'s
    /// `SchedulerEvent::Dispatched`). No-op once `ready`. If every bucket is
    /// currently occupied (more concurrently in-flight real dispatches than
    /// `n_virtual` — see the struct doc), this dispatch simply isn't
    /// tracked; its eventual `record` falls back to the least-busy path.
    pub(super) fn record_dispatch(&mut self, instance_id: i64, dispatch_time_abs: f64) {
        if self.ready {
            return;
        }
        let t0 = *self.t0.get_or_insert(dispatch_time_abs);
        let rel = dispatch_time_abs - t0;
        if let Some(w) = self.least_busy_free_bucket() {
            self.pending.insert(w, (instance_id, rel));
            self.assigned.insert(instance_id, w);
        }
        self.refresh(rel);
    }

    /// Feed one more real (or cached) result. No-op once `ready`.
    pub(super) fn record(&mut self, instance_id: i64, runtime: f64) {
        if self.ready {
            return;
        }
        let (w, finish) = if let Some(w) = self.assigned.remove(&instance_id) {
            let (_, dispatch_rel) = self
                .pending
                .remove(&w)
                .expect("a bucket in `assigned` always has a matching `pending` entry");
            (w, dispatch_rel + runtime)
        } else {
            let w = self
                .virtual_busy
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map(|(w, _)| w)
                .expect("virtual_busy is never empty");
            (w, self.virtual_busy[w] + runtime)
        };
        if finish <= self.checkpoint_time {
            self.time_sum += runtime;
            self.n_counted += 1;
            if runtime < self.cutoff_time {
                self.solved_count += 1;
            }
        }
        self.virtual_busy[w] = finish;
        self.n_seen += 1;
        if self.n_seen == self.n_total {
            // Every instance is real-accounted-for now. Return before
            // computing `ready` at all -- even if every virtual worker
            // happens to already be past the horizon at this exact instant,
            // `score()` treats `n_seen == n_total` as "already exact" (see
            // below), so there is no case where setting `ready` here would
            // ever produce a usable answer.
            return;
        }
        // `finish` is a valid lower bound on this neighbour's own relative
        // "now": a completion can only be observed after its own relative
        // finish time has actually elapsed. Using it here means `record`
        // never needs a separately-threaded "current time" parameter.
        self.refresh(finish);
    }

    /// Re-check readiness using the current real clock, for when no new
    /// dispatch/completion has arrived to trigger it on its own (e.g. every
    /// virtual bucket is occupied by a real instance still running, with
    /// nothing else queued to report progress in the meantime) — called
    /// from the consumer loop's recv-timeout branch. No-op if this
    /// neighbour hasn't seen its first real dispatch yet (`t0` unset),
    /// since there is nothing pending to reassess in that case.
    pub(super) fn poll(&mut self, now_abs: f64) {
        if let Some(t0) = self.t0 {
            self.refresh(now_abs - t0);
        }
    }

    /// `now_rel` must be a valid lower bound on this neighbour's own current
    /// relative time (never an overestimate) — both call sites above
    /// guarantee that.
    fn refresh(&mut self, now_rel: f64) {
        if self.ready || self.n_seen >= self.n_total {
            return;
        }
        let all_past_horizon = (0..self.virtual_busy.len()).all(|w| {
            if self.pending.contains_key(&w) {
                // Still running: hasn't finished as of `now_rel`, and
                // `now_rel` already exceeds the horizon, so its eventual
                // finish (>= now_rel) must too -- regardless of its
                // eventual runtime.
                now_rel > self.checkpoint_time
            } else {
                self.virtual_busy[w] > self.checkpoint_time
            }
        });
        if all_past_horizon {
            self.ready = true;
        }
    }

    /// `Some(n_total - solved_count)` only for a config that is **still in
    /// flight** when the virtual horizon matures (`ready` with `n_seen <
    /// n_total`) -- a genuine early read; lower is better. `None` in every
    /// other case, including when `n_seen` reaches `n_total` *before*
    /// `ready` -- that means the real evaluation already finished, so its
    /// exact score is already available through the normal completion path,
    /// and the checkpoint has nothing to add.
    pub(super) fn score(&self) -> Option<f64> {
        if !self.ready || self.n_seen >= self.n_total {
            return None;
        }
        Some((self.n_total - self.solved_count) as f64)
    }

    /// PAR1 estimate at the checkpoint: `time_sum` (real, PAR1-contract
    /// runtimes for every result that virtually finished by
    /// `checkpoint_time`) plus `cutoff_time` for each of the remaining
    /// `n_total - n_counted` instances -- the same "treat what hasn't been
    /// seen yet as a timeout" assumption `score()` makes for solved_count,
    /// just carried through as a mean runtime instead of a count. Same
    /// guard as `score()`; logged alongside it purely for comparison, not
    /// yet consulted by D8's reject rule.
    pub(super) fn par1(&self) -> Option<f64> {
        if !self.ready || self.n_seen >= self.n_total {
            return None;
        }
        let uncounted = self.n_total - self.n_counted;
        Some((self.time_sum + uncounted as f64 * self.cutoff_time) / self.n_total as f64)
    }
}

/// Whether future-telling should build/consult a `CheckpointTracker` at all
/// this round (D3). Below this line the mechanism is provably inert — every
/// arriving instance finds an idle virtual worker, so `ready` can never fire
/// before the config's real evaluation is already fully complete — so
/// running it would spend bookkeeping for zero possible benefit.
pub(super) fn future_telling_active(options: &IlsOptions, n_instances: usize) -> bool {
    options.future_telling && n_instances > options.future_telling_cores
}

/// Reject rule (D8), same shape as `accepted_within_tolerance` but inverted
/// for a reject: fire only when the challenger's checkpoint (an *unsolved*
/// count — lower is better, D6) exceeds the incumbent's own by more than the
/// tolerance band. Takes plain `f64`, not `Option<f64>` — the caller only
/// ever calls this after unwrapping both sides (D7/D8); "reference missing"
/// is handled once, at the call site.
pub(super) fn future_telling_rejects(challenger_ckpt: f64, incumbent_ckpt: f64, options: &IlsOptions) -> bool {
    challenger_ckpt.is_finite()
        && incumbent_ckpt.is_finite()
        && challenger_ckpt > incumbent_ckpt + options.future_telling_tolerance * incumbent_ckpt.abs()
}
