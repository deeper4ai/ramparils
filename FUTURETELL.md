# FUTURETELL — checkpoint-based early rejection for BLS neighbours

Status: **Implemented end to end and wired into `run()`/`basic_local_search`/
`collect_one`, default off (`future_telling: false`).** Validation plan
items 1–5 are done; only item 6 (a real tuning comparison, before ever
flipping a default) remains, deliberately out of scope for this
implementation pass — see "Staged implementation checklist" and "What the
integration tests actually show" below for what item 5 ended up proving and
why its original framing changed along the way. Name is a placeholder —
happy to rename.

## Background

`work26/expericon/ramparils-eprover` spent 2026-09-11/12 studying whether a
cheap early read of a config's evaluation predicts its expensive final score
well enough to act on. Summary of what was established there (full detail in
that experiment's `DIARY.md`):

- Given a config's evaluation on a fixed instance pool with `N` parallel
  workers, the number of instances that have **already returned with a
  successful status** after only `T` seconds of wall-clock time (a
  "checkpoint") predicts the config's eventual full score well — 93.6%
  pairwise accuracy at `T=5s` against `train2k`'s own true final ranking,
  climbing further against a *different*, much larger benchmark
  (`train120k`, 94.7% at the same checkpoint).
- The signal has a known failure mode: it's much sharper for large score
  gaps than for close ones. Two configs that are nearly tied on the
  checkpoint are usually also nearly tied on the final score — checkpoint
  disagreement clusters almost entirely among the closest pairs. This
  matters directly for the "reject early" use case: the feature must be
  wrong *slowly*, i.e. only pull the trigger on gaps clearly outside noise.
- Counting *any* return (solved or failed) rather than *successful* returns
  is a measurably weaker signal ("time-to-N-done" topped out well below the
  solved-count checkpoint at every matched sample size) — a config that
  fails fast looks good on a pure-throughput read. The checkpoint metric
  must be sensitive to *outcome*, not just *speed*.
- A **simulated N-worker replay** (list-scheduling: N workers, each idle one
  takes the next queued job; feed in real per-instance runtimes) reproduces
  a real batch's own checkpoint values exactly at the boundaries, and lets
  the checkpoint be evaluated at *any* virtual worker count for free —
  larger virtual `N` gives a **sharper** signal (98.3% accuracy at simulated
  N=256 on the same data that gave 93.6% at the real N=64), because more
  virtual parallelism spreads out the "everyone hits the identical cutoff at
  once" tie pile-up that a small worker count produces.

We decided (previous turn, this conversation) to build the RamPaRILS-side
feature on **simulation**, not on real wall-clock time, specifically so the
virtual worker count is a free parameter decoupled from `cores:` — and to
implement it in the one shared code path both `Approach::Basic` and
`Approach::Focused` go through, rather than special-casing either (see
"Design decisions" below).

## What this feature is

During `basic_local_search`'s per-round neighbour evaluation, and during any
single-config evaluation (`collect_one`), reject a config outright — the
same "stop watching it, count it as done, move on" outcome adaptive capping
already produces — once a **simulated checkpoint score**, computed from the
real per-instance results that have streamed back so far, is significantly
worse than the same checkpoint computed for the incumbent. This is a second,
earlier, *heuristic* prune sitting alongside the existing adaptive-capping
prune, which is a *proof*. The two are complementary, not a replacement for
one another (see "Relationship to adaptive capping" below).

## How evaluation works today (context for the design)

Recapped in more detail in this conversation; the load-bearing facts for
this design:

- `Scheduler` (`src/eval.rs`) parallelizes over `(neighbour, instance)`
  pairs across `n_workers` (`cores:`) threads, dispatching a neighbour's
  instances in fixed array order via a shared atomic index
  (`WorkBatch::next_index`), but **real completion order is not index
  order** — runtimes vary per instance, so results stream back in whatever
  order the solver processes finish.
- `basic_local_search` (`src/ils.rs:953`) submits every neighbour of the
  current config as a separate `EvalTask` in one `scheduler.submit()` call,
  then loops on `scheduler.results()`, updating one set of per-neighbour
  accumulators (`runtimes[nid]`, `qualities[nid]`, `partial[nid]`,
  `done[nid]`) as results arrive, in **arrival order**, regardless of which
  neighbour they belong to.
- Adaptive capping (`ils.rs:1070-1092`) is the existing prune: once a
  neighbour's running cost sum exceeds `bound_multiplier * incumbent_score *
  n_instances`, it's marked `done[nid] = true` and skipped from then on.
  This is a **mathematical proof** — costs never decrease, so exceeding the
  budget proves the eventual mean must too.
- `collect_one` (`ils.rs:1163`) is the single-config counterpart, used for
  the initial config, the perturbed candidate at the top of every outer-loop
  round, restart candidates, and fidelity re-measurement of the incumbent
  and home base. It has the same capping check, structurally identical.
- `Approach::Focused` and `Approach::Basic` share **the same** `run()` and
  `basic_local_search()`; the only difference is what `n_runs` is set to
  before each round (`ils.rs:251-254`, grown via `next_n_runs` for
  `Focused`). There is no separate focused-specific descent loop.
- `compute_score` (`ils.rs:1251`) aggregates whichever of `runtimes`/
  `qualities` matches `options.run_obj`, by mean or median
  (`options.overall_obj`). This is what both the real score and (this
  proposal's) checkpoint score should compute over — no new "solved/not
  solved" concept needs importing; RamParILS already has a notion of
  per-instance value and a way to aggregate it.

## Design decisions

**D1 — Simulate, don't read real wall-clock.** Confirmed with the user.
Reading a real elapsed-time checkpoint would conflate the checkpoint with
however many *other* neighbours happen to be sharing the worker pool at that
moment (real contention is only absent when `n_runs >= n_workers`, per this
conversation's earlier analysis) and would tie the checkpoint's virtual
worker count to `cores:`, forfeiting the "more virtual workers, sharper
signal" finding above.

**D2 — No `Approach` special-casing.** Since `Focused` and `Basic` share one
descent function, the checkpoint hook goes in that one function (and in
`collect_one`, its single-config sibling) with no `options.approach` branch.
`Focused`'s early low-fidelity rounds naturally end up excluded anyway — see
D3, which achieves this with a plain numeric guard rather than a
`Focused`-specific carve-out.

**D3 — Gate the entire mechanism on `n_runs > future_telling_cores`.**
Not a scope restriction for convenience — the feature is provably inert
below that line. When `n_runs <= future_telling_cores`, every arriving
instance finds an idle virtual worker (more workers than jobs), so most
virtual workers never get assigned anything and sit at busy time `0.0`
forever. `ready` (originally: "every virtual worker's busy time exceeds
`checkpoint_time`") can then **never fire before the config's real
evaluation is already fully complete** — an idle worker at `0.0` is always
`≤ checkpoint_time`. So in that regime the checkpoint cannot mature before
there's nothing left to reject early *anyway*; running the mechanism there
would spend bookkeeping for zero possible benefit, not merely reduced
benefit.

This is checked once per round, before any `CheckpointTracker` is built —
`n_runs` and `options.future_telling_cores` are both already in scope at
that point in both `collect_one` and `basic_local_search`, so it's one
integer comparison wrapping the whole per-round setup, no new state and no
`Approach` awareness needed. It happens to exclude `Focused`'s early
low-fidelity rounds (`n_runs` as low as 1, `initial_n_runs`, `ils.rs:1279`),
but the same guard would equally exclude a `Basic`/`Random` run whose entire
instance pool is smaller than `future_telling_cores` — one rule, not a
per-approach one, which is what actually delivers on D2's intent.

Worth flagging as a real coupling, not a flaw: `future_telling_cores`
now does double duty — larger values sharpen the signal (the diary's own
finding) but also raise the bar for how far into `Focused`'s fidelity growth
the feature has to reach before it activates at all. See "Open questions."

**D4 — Fixed instance-index order for the virtual schedule, not arrival
order.** **Superseded again, 2026-09-13, by D10 below — kept here as the
record of why fixed order was tried first, not as the current design.**
Superseded from an earlier draft of this document after checking
`expericon/ramparils-eprover/DIARY.md` 2026-09-11 ("alphabetical-order
confound found; `train2k.rnd` and `shuffle: false` fix"): solverpy was found
to reshuffle task-launch order independently *per strategy*, unseeded,
every evaluation (`benchmark/evaluation.py:51,148-150`) — so two strategies
being compared at a checkpoint could each get their own random draw of
which instances landed early, which is exactly what produced a spurious
"slower but thorough" reading for one strategy that turned out to be nothing
but a lucky shuffle. The fix adopted there, `shuffle: false`, forces one
shared, fixed instance order across every strategy being compared — a
checkpoint comparison is only meaningful when every side is racing over the
*same* queue.

Checked whether RamParILS has the same exposure: it doesn't, and doesn't
need a `shuffle: false`-equivalent fix, because there's no shuffle to
disable. `grep -rn shuffle src/` in `ramparils` returns nothing;
`load_instances` (`scenario.rs:441`) reads the instance file's lines
verbatim, no sort, no randomization. `instances: &[(i64, String)]` is
threaded unchanged from `run()`'s caller down; `eval_instances =
&instances[..n_runs]` (`ils.rs:966`) is the same slice, same order, shared
via one `Arc` across every neighbour's `EvalTask` in a round (`ils.rs:992`),
and every round for the life of the run reuses a growing prefix of that same
fixed array. Index 0 is the same instance, at the same position, for the
initial config, every neighbour ever evaluated, the incumbent, and the home
base — stricter than solverpy's per-batch fix, since it holds for the whole
run, not just one comparison. So the checkpoint simulation should use this
existing fixed order directly, replicating the `expericon` replay's method
exactly rather than approximating it.

**Consequence: the tracker needs to know each result's fixed position, and
advance its simulation cursor through a contiguous prefix, not just append
in arrival order.** Real per-instance results still stream back in whatever
order the solver processes happen to finish (workers execute at different
real speeds even though they *claim* indices in order — see
`eval.rs:151-190`), so a result for index 40 can easily arrive before index
3. The ILS side already holds `eval_instances` for the round, so it can
build a `HashMap<i64, usize>` (`instance_id` → fixed position) locally, with
**no changes needed to `eval.rs` or `TaskResult`** — this stays entirely
inside `ils.rs`. The tracker buffers arrived-but-not-yet-consumable results
keyed by position, and each new arrival advances a `cursor` through as many
now-contiguous positions as are available (one arrival can unblock several
buffered ones at once if they'd arrived earlier out of order).

**Real cost of doing this correctly**: if a low-index instance happens to be
a slow, near-cutoff one, the simulation cursor stalls there until that
specific real result finally lands, even if dozens of higher-index instances
already finished. This can materially delay when a neighbour's checkpoint
matures, independent of whether that neighbour is actually good or bad —
purely a function of where the hard instances happen to sit in the instance
file. **This is more than a slow-checkpoint annoyance — see D5**: if
difficulty correlates with position across a wide stretch of the array (not
just one unlucky instance), maturity can be delayed until nearly the whole
evaluation is already done, which defeats the point of an *early* reject
entirely. Fixed by D5's global instance shuffle, not left as a documentation
footnote — an earlier draft of this section proposed "tell users to
pre-shuffle their own instance file" as the mitigation; that was rejected in
favor of RamParILS doing it itself, for the same reason `expericon`'s
`shuffle: false` treats "the user might forget" as a real risk rather than
an acceptable one.

**D5 — The fixed order (D4) must also be decorrelated from difficulty, or
the checkpoint fires too late to help; solved with a global, once-only
instance shuffle in `run()`, not a future-telling-internal one.** **Future-
telling itself no longer depends on this as of D10 (2026-09-13) — arrival
order has no "difficulty-correlated position" to decorrelate. `instance_shuffle`
is kept regardless, default `true`, for the reason D10 restates: it still
decorrelates FocusedILS's own fidelity-prefix growth from any difficulty
ordering in the instance file, entirely independent of future-telling.**
Caught
during review of this document, and it invalidated an earlier version of
this same decision: initially proposed a shuffle *scoped to future-telling's
own internal position bookkeeping*, leaving real dispatch untouched. That
doesn't work. Real dispatch pulls instances via `next_index.fetch_add(1)`
(`eval.rs:151-190`) in strict real-array-index order, so real *arrival*
timing follows real-array-index order too (up to per-instance runtime
variance) — an internal-only remap decorrelates future-telling's position
numbers from difficulty, but the *cursor's ability to advance* is still
gated by real arrival, which is still governed by the *unshuffled* array.
If future-telling's "position 0" happens to map to real-array-index 8000 of
10000, the cursor cannot advance past it until that specific instance's real
result lands — which, since real dispatch proceeds through the array in
order, may not happen until nearly everything else is already done. Fixing
*correctness* (D4) while leaving *timing* broken defeats the whole feature.

**Fix: shuffle the real `instances` array itself, once, before `run()` does
anything else with it** — then dispatch order and future-telling's position
numbering are the same thing again, and D4's original design (position =
real array index) applies completely unmodified; no separate remapping
layer, no per-fidelity-epoch reshuffling, no change to `CheckpointTracker`
at all. Concretely: right where `n_total = instances.len()` is already
computed (`ils.rs:248`), build an owned, shuffled copy
(`instances.to_vec()` + a seeded `.shuffle(&mut rng)`) when
`options.instance_shuffle` is set, and use that copy for the rest of
`run()` — dispatch, `Focused`'s fidelity prefixes, and future-telling's
positions all flow from the same shuffled array from that point on. Stays
entirely inside `ils.rs`, same as D4 originally claimed — the shuffle just
moved one level up, from `CheckpointTracker` to `run()` itself.

**Ordering requirement**: the shuffle must happen *after* `cache.load_instances()`
has already assigned each path its `instance_id`, never before. Instance ID
assignment must stay independent of shuffle settings — a given path's
`instance_id`, and therefore its cache entries, must never depend on
whether or how shuffling is configured. The shuffle is a pure reordering of
an already-ID-assigned `Vec<(i64, String)>`, nothing more.

**New scenario options, top-level, not `future_telling`-prefixed** — this is
general RamParILS instance-handling infrastructure future-telling depends
on, not something future-telling owns:

```yaml
instance_shuffle: true       # default on; shuffle instances once before dispatch
instance_shuffle_seed: 0     # deterministic by default, same reasoning as elsewhere
```

Default `true` — an experienced user can turn it off if they have a reason
(their own already-randomized instance file, e.g. `expericon`'s own
`train2k.rnd` convention, where a second shuffle would be redundant, or a
deliberate wish to preserve literal file order for some other purpose).
This supersedes the earlier, narrower `future_telling_seed`/
`future_telling_shuffle` idea floated for this same problem — once the real
array itself is what's shuffled, future-telling has no remaining randomness
of its own to seed (`CheckpointTracker`'s "least-busy worker" tie-break in
`record()` is a deterministic `min_by`), so a second, feature-specific seed
would only add redundant surface area.

**Blast radius, stated plainly, not smoothed over**: unlike everything else
in this document (gated behind `future_telling: false` by default),
`instance_shuffle: true` changes real dispatch order for *every* scenario
that doesn't explicitly opt out — including ones that never touch
`future_telling` at all. Justified because it's deterministic (a given
scenario + seed still reproduces identically across reruns, just with a
different, still-fixed order than literal file order), it doesn't touch
cache validity (per the ordering requirement above), and — a genuine bonus,
not the primary motivation — it also fixes a previously-unaddressed,
pre-existing bias risk in `Focused`'s own fidelity-prefix growth:
`instances[..n_runs]` for a small `n_runs` was exactly as exposed to
alphabetical/family bias as future-telling's checkpoint, independent of
this feature entirely, and this happens to fix both at once. Still a
deliberate default-behavior change for users who never asked for one, and
worth flagging as such in the `CHANGELOG.md` entry, not folded silently
into "added future-telling."

**`future_telling: true` with `instance_shuffle: false` gets a startup
warning, not a hard error.** Not always wrong — a user with a genuinely
pre-randomized instance file has a legitimate reason for this combination —
and RamParILS has no way to inspect whether a given instance file is
already effectively random or difficulty-correlated by position, so an
error would block a valid usage pattern it can't actually distinguish from
a broken one. A warning naming the risk explicitly is the right level of
intervention: it makes the assumption visible, which is the whole lesson
carried over from missing solverpy's own unseeded shuffle in the first
place, without pretending RamParILS can verify something it can't.

**Debug logging**: record that the shuffle happened and with what seed —
e.g. `[t] ils: instance_shuffle applied to N instances (seed=S)`, once per
run, at the point the shuffle runs. Matches this project's own provenance
conventions and, again, is precisely the "never silent" property the
`expericon` mistake this is modeled on was missing.

**D6 — Checkpoint score is a solved-count (via the existing PAR1 wrapper
contract), not a `compute_score` aggregate.** Superseded from an earlier
draft of this document, which reused `compute_score` (mean/median of
`runtime`/`quality`) specifically to avoid inventing a "solved" concept
RamParILS doesn't otherwise have. That reasoning was sound but the
conclusion was wrong, for a reason found only by tracing it through
carefully: **`compute_score` is volume-*insensitive* by construction** — a
mean or median describes central tendency, completely independent of how
many instances contributed to it. The diary's validated signal was a raw
**count** — more solved by T is better, full stop — and a statistic that
can't tell "3 excellent completions" from "500 decent ones" apart is not a
port of that signal, it's a different one, untested against anything. Caught
before implementation, not after: see "Rejected: `compute_score` (mean/median
of runtime/quality)" in the Validation plan for the concrete failure case.

**The fix turns out not to need a new domain concept after all.** Checking
`docs/reference/protocol.md`: the wrapper contract already **mandates PAR1
unconditionally**, for any non-success outcome, regardless of `run_obj` —
"`runtime` on a non-success line must be `cutoff_time`, not whatever time the
process actually took... a wrapper that reports a crash's true near-instant
runtime makes crashing look like the *best* possible outcome." The worked
TPTP example confirms this covers exactly the case that looked like a risk
(`GaveUp`, a heuristic early exit that isn't a genuine solve) — it reports
`runtime = cutoff_time`, identical to `Timeout`/`ResourceOut`/`UNKNOWN`. That
means **`runtime < cutoff_time` is an exact match for "solved,"** not an
approximation, for any wrapper following a contract this project already
requires and has a documented incident about violating (the 43%-error
episode in `protocol.md`) — no new wrapper field, no status-string matching,
no closed vocabulary list to maintain. (`src/db.rs`'s existing
`SOLVED_STATUSES`/`is_solved()` does the same classification via the raw
status string instead, for the offline `db solved` export — the two should
agree for a compliant wrapper; a generalized `run_obj: solved` reusing that
machinery is now filed separately, `TODO.md`, "Proposed — `run_obj:
solved`," out of scope here.)

**Checkpoint score = count of instances *not yet confirmed solved*, out of
the full `n_runs`** — i.e. `n_runs - solved_count`, where `solved_count`
counts instances that (a) virtually finished by the checkpoint horizon
*and* (b) have `runtime < cutoff_time`. Lower is better, consistent with
every other score in this codebase, and correctly volume-sensitive: solving
more (regardless of how many total instances have returned) lowers the
count; instances that haven't returned yet count against the total exactly
like ones that returned and failed — no separate placeholder value needed,
because a plain count doesn't require one the way an aggregate would. This
also has no dependency on `run_obj`/`overall_obj` at all — a config's
checkpoint score is the same number no matter whether the *real* score is
being tuned for runtime or quality, which is correct: "how much is
confirmed solved" is a fact about the domain, not about which objective the
user chose to optimize.

**D7 — Reference values are in-memory run state, not persisted to the
cache; the incumbent's is overwritten unconditionally, including to
`None`.** "Remember the incumbent's/home base's checkpoint value" doesn't
need a schema change: the moment a config's evaluation streams through
either collection loop, that same loop already builds up its checkpoint
score for free (it needs the same accumulation regardless of whether this
config wins). When a config becomes the new incumbent or new home base
(`ils.rs:360-365`, `468-474`, `496-518`, `587`, `654-663`), stash its
checkpoint score into two new `run()`-local variables,
`incumbent_checkpoint: Option<f64>` and `home_base_checkpoint: Option<f64>`,
threaded through the same call sites that already thread
`incumbent_score: Option<f64>` for adaptive capping. No SQLite involvement,
no cross-run persistence — this is scoped to one `run()` call, same
lifetime as `incumbent_score` itself.

**`incumbent_checkpoint` will often be `None`, and that's fine — the
reassignment must happen anyway, every time, even when the new value is
`None`.** Two independent reasons it can be missing: D3's structural gate
(`n_runs <= future_telling_cores` that round, e.g. `Focused`'s early
fidelity), and the "config finished before its checkpoint matured" case (a
per-config outcome — a fast-solving config's own tracker never crosses the
horizon before `n_seen` reaches `n_total`, `CheckpointTracker::score()`
below). The second case is *structurally* likely to hit the incumbent
specifically: whatever neighbour wins and becomes the new incumbent, by
construction, **fully completed** its evaluation — a capped or
checkpoint-rejected neighbour never reaches the "compare and maybe replace"
branch at all — so every incumbent replacement is a config whose own
`n_seen` reached `n_total`, which is exactly the condition under which
`score()` returns `None`. A cache-heavy re-evaluation (e.g. the fidelity
re-measurement in `ils.rs:596-605`) makes this more likely still: the
scheduler delivers cache hits into the result stream synchronously, all at
once, before any live solver call returns (`eval.rs:220-276`), which looks
to the tracker exactly like an unusually fast config.

**This is an accepted consequence, not a gap to design around**: the
feature exists to speed up search over a large instance count by cutting
off configs early; a config (or round) that finishes fast has nothing left
to speed up, so there being no checkpoint verdict to act on there costs
nothing. The only design obligation this creates is at the *update* site:
`incumbent_checkpoint` must be reassigned from the current evaluation's
tracker unconditionally, every time `incumbent_score` is — **including
overwriting a previous `Some` with a fresh `None`.** Keeping the old value
when the new one is absent would compare a future challenger against a
stale reference describing a config that is no longer the incumbent, which
is exactly the kind of quiet incorrectness this design guards against
elsewhere too. The feature simply goes quiet (never rejects) for stretches where
`incumbent_checkpoint` is `None`, and resumes the moment some future
incumbent's own checkpoint happens to mature — self-healing, not a bug to
catch.

**D8 — Same rejection mechanism as adaptive capping, different trigger; a
missing reference means skip, exactly like the existing capping check.**
"Reject and continue with the next one" in the BLS neighbour loop is
already exactly what `done[nid] = true; n_done += 1; continue;` does. In
`collect_one`, an early stop already produces `ConfigEvaluation { complete:
false, .. }`, which every caller already treats as a lower bound, not a
measurement. The checkpoint prune is a second `if` beside the existing
budget check at both sites (`ils.rs:1075-1092` and `ils.rs:1224-1237`) —
no new control-flow shape, no new outcome type. And like that existing
check (`if let Some(inc) = incumbent_score { .. }`), the checkpoint check
only ever runs when *both* sides are available — `incumbent_checkpoint` is
`Some` (D7) *and* this challenger's own `tracker.score()` has matured —
call site shape: `if let (Some(inc), Some(chal)) = (incumbent_checkpoint,
tracker.score(options)) { if future_telling_rejects(chal, inc, options) {
.. } }`. Missing either side is not an error case to handle, it's the
default "nothing to compare yet" state.

**D9 — Compare against the incumbent's checkpoint, not the home base's.**
The existing adaptive-capping budget is anchored on `incumbent_score`
specifically (not `last_lm`/home base) at every call site that threads it.
For consistency — and because that's the quantity the same call sites
already have in scope — the checkpoint prune compares a challenger's
checkpoint score against `incumbent_checkpoint`. `home_base_checkpoint` is
still recorded (D7) since it's nearly free once the incumbent one is
threaded through, and it may turn out useful for the acceptance side (e.g.
a future cheap-reject on `acceptance_criterion`'s "not worth a full
re-evaluation against the home base" case) — but nothing in this design
depends on it yet. Flagged as an open question below rather than assumed.

**D10 — Arrival order, not fixed order; supersedes D4/D5's fixed-order
requirement (2026-09-13).** Found by tracing down a real, measured problem,
not by re-deriving the design in the abstract: `ramparils-eprover` RUN 06
(2026-09-13, `future_telling_checkpoint: 1.0`, `cutoff_time: 5.0`, `cores:
64`) showed a checkpoint taking **~20s of wall clock to mature when direct
replay of the actual recorded runtimes showed it only ever needed 149
(fixed-order) instances**. Root cause: D4's design requires the cursor to
advance through a *strictly contiguous* fixed-order prefix, buffering
anything that arrives out of order. Real dispatch pulls from the same
shared index D4 assumed it would track, but per-instance runtime variance
means completion order drifts from index order — a handful of slow
instances sitting early in the fixed order can leave the cursor blocked
while hundreds of *later*, already-arrived results sit unused in the
buffer. The tracker was, in effect, refusing to look at information it
already had.

**Fix: drop position and the fixed order entirely. `CheckpointTracker::record`
now takes only `runtime`, and assigns each result, as it actually arrives,
to whichever virtual worker is currently least busy** — the same list-
scheduling assignment rule as before, just driven by real arrival order
instead of a buffered replay of a hypothetical fixed schedule. No `pending`
buffer, no `cursor`, no instance identity or position map needed anywhere
(the `(instance_id -> position)` map D4 required, and the code that built
it in `basic_local_search`/`collect_one`, are gone). This directly
addresses the RUN 06 delay: the tracker now advances the moment each real
result lands, never blocked on a specific straggler while other data sits
idle.

**Cost, stated plainly: this is no longer arrival-order invariant.** D4's
fixed-order design guaranteed the same multiset of runtimes produced the
same maturity point and score regardless of delivery order — useful for
testing, and for exactly reproducing the diary's own published reference
numbers under any simulated delivery order. That property is gone
(`checkpoint_tracker_now_depends_on_arrival_order_by_design` in
`src/ils.rs`'s test module demonstrates it directly, replacing the old
arrival-order-invariance test). What replaces it: the tracker's maturity
point and score now depend on real completion timing, the same sensitivity
the real evaluation it approximates already has — which is the more
honest trade for a mechanism whose whole job is reading that real timing
as early as possible. The replay tests against the diary's fixtures
(`checkpoint_tracker_replay_*`) still pass unchanged, since they replay the
fixture's own one recorded order rather than testing invariance across
several.

**D5's decorrelation concern (difficulty correlated with fixed position)
no longer applies to future-telling itself** — there is no fixed position
left to correlate with anything. `instance_shuffle` stays on by default
regardless, for the independent reason D5 already flagged as a bonus:
decorrelating FocusedILS's own fidelity-prefix growth from instance-file
ordering. The `future_telling: true` + `instance_shuffle: false` startup
warning (D5) is now stale specifically *for future-telling's own sake* and
could be dropped from that rationale — left in place since it costs nothing
and instance_shuffle is still recommended on general grounds.

**D11 — Virtual busy-time from dispatch events, not arrival; `future_telling_cores`
defaults to and is capped in spirit at real `cores`, never enforced above it
(2026-09-13).** Resolves the "under research, not implemented" open question
below, worked out against a small hand-simulated example
(`work26/expericon/ramparils-eprover` conversation, 2026-09-13) before
touching any code, same as D10's real-bug-first method.

**Problem restated**: D10's `record()` only learns a real worker was busy
*after* its result arrives, so it reconstructs busy-time retrospectively
from summed completed runtimes. On a timeout-heavy config this
systematically overstates how long maturity should take: a real worker
that has been running a doomed instance for 4.9s is, for every practical
purpose, already "spent" for this checkpoint — but the tracker doesn't
know that until the full 5s elapses *and* the result is delivered. Measured
consequence (`ramparils-eprover` RUN 06 v2, `DIARY.md` 2026-09-13): 64%
timeout rate, nominal 5s horizon, actual maturity 10.6s — filling all 64
virtual buckets took ~164 arrivals (~2.6 "waves") instead of one.

**Fix: a real worker's busy-time should count from when it *starts* an
instance, not from when the result comes back.** Requires a new signal from
`eval.rs`: the worker loop already knows the exact moment it's about to run
an instance (`eval.rs`, right after `batch.next_index.fetch_add(1, ...)`,
before calling `run_solver_inner`) — emit a lightweight dispatch event there
(`batch_id`, `neighbor_id`, `instance_id`, the real worker's own index, and
`crate::t()` at that moment).

**Same channel as `TaskResult`, not a second one — the existing
`Sender<TaskResult>`/`Receiver<TaskResult>` pair becomes
`Sender<SchedulerEvent>`/`Receiver<SchedulerEvent>`, where**

```rust
enum SchedulerEvent {
    Dispatched { batch_id: u64, neighbor_id: usize, instance_id: i64, dispatch_time: f64 },
    Completed(TaskResult),
}
```

Considered and rejected: a parallel dispatch channel consumed via
`crossbeam::select!` in `ils.rs`. Not needed — the reason a *new signal* is
needed at all is that a dispatch event has none of `TaskResult`'s fields
(`runtime`/`quality`/`status`/`runhash`/`cacheable` only exist once a solver
has actually finished), not that it needs a separate transport. A dispatch
notice smuggled through as a fake `TaskResult` (placeholder runtime) would
be misread by the existing consumer logic in `collect_one`/
`basic_local_search` as a genuine completion — wrongly advancing
`runtimes[nid]`/`n_seen`, tripping the full-completion check early, and
corrupting adaptive capping's cost sum. The enum keeps one channel, one
receive loop, and makes the two cases impossible to conflate: the consumer
matches `Dispatched` to `CheckpointTracker::record_dispatch` and routes
`Completed(result)` through the existing pipeline (capping →
full-completion → `record`/checkpoint) exactly as today, no `select!`
anywhere. Per-sender ordering on the shared channel also guarantees a
worker's `Dispatched` for an instance is always seen before its matching
`Completed`, so the tracker never has to reconcile timestamps across two
independent streams. `CheckpointTracker` gains a
`record_dispatch(real_worker_id, dispatch_time)` used to seed/advance that
worker's virtual busy-time immediately; `record(runtime)` (unchanged
signature) settles it exactly once the real result lands — whichever
happens to update `ready` first, a worker whose `now - dispatch_time >=
checkpoint_time` is already known-timed-out before its result physically
arrives, and should not block maturity.

**This only produces a clean bound when there's a 1:1 mapping between
virtual buckets and real workers — i.e. `future_telling_cores == cores`,
chunk size 1 in the sense worked out in conversation.** Every real worker's
*first* dispatch happens at (approximately) the same instant, so the
slowest of them settles by `checkpoint_time` at the latest — that's what
makes "5s + a few ms of signal overhead" achievable at all. The moment
`future_telling_cores > cores`, there physically aren't enough real workers
to give every virtual bucket a dispatch at t≈0: the excess buckets can only
be filled once a real worker frees up from its *first* instance, which, in
the worst case (every first-wave instance a timeout), doesn't happen until
`checkpoint_time` has already elapsed once — so those buckets' own deadlines
land at `2 * checkpoint_time`, not `checkpoint_time`. In general the bound
degrades to `ceil(future_telling_cores / cores) * checkpoint_time`, the same
shape as the original queue-contention problem, just re-introduced through
oversubscription instead of unlucky ordering.

**Decision: `future_telling_cores` defaults to `cores` (already true, D3/
"New scenario options" — `null` resolves to `options.n_workers`) and setting
it explicitly higher than `cores` is allowed, not rejected or capped.** A
user chasing the diary's "more virtual workers, sharper signal" finding gets
exactly that signal, at the cost of slower maturity in proportion to the
oversubscription ratio — a real, known trade-off, not a bug to guard
against. What changes here is only that this trade-off must be stated
explicitly (in this doc and in the scenario option's own doc comment) rather
than left implicit: raising `future_telling_cores` above `cores` does not
make the checkpoint arrive sooner or the signal free — it can only make
maturity slower, in the multiples-of-`checkpoint_time` sense above. No
startup warning is proposed for this (unlike D5's `instance_shuffle`
warning) — the effect is monotonic and self-correcting (a user who sees
maturity taking noticeably longer than expected has an immediate, legible
explanation), not a silent correctness risk.

**Scope check against D3's existing gate**: D3 already requires
`n_runs > future_telling_cores` for the mechanism to activate at all: raising
`future_telling_cores` for a sharper signal also raises this bar, on top of
the new slower-maturity cost above — both costs of the same knob, worth
restating together rather than only in the older "Open questions" note.

Bucket assignment when `future_telling_cores < cores` (more real workers than
virtual buckets): `record_dispatch`'s "least-busy free virtual bucket"
assignment (same rule as D10's arrival-time version) handles this correctly
as a many-to-one mapping without any special-casing — several real workers'
dispatches simply land on the same bucket over time, same as today.

**Resolved during implementation, 2026-09-13: what is `dispatch_time`
relative to?** `checkpoint_time` is an absolute duration, but real dispatch
times live on the run's global clock (`crate::t()`), which climbs across the
whole tuning run — so a reference point is needed. Two candidates, both
worked through against `basic_local_search`'s *actual* dispatch mechanics,
not assumed in the abstract:

- **Anchor to the round's `submit()` call** — one shared zero for every
  neighbour in the round. Rejected: `submit()` pushes each neighbour's
  `WorkBatch` tokens onto the *same* shared channel one neighbour at a time
  (`eval.rs`'s `for task in tasks` loop, `worker_slots` copies per
  neighbour) — confirmed empirically with a throwaway test (4 real workers,
  2 neighbours, 8 instances each at 0.3s): neighbour 1 got **zero** workers
  until neighbour 0's batch was completely drained. Anchoring to `submit()`
  would count that queue-wait as if it were spent against a *later*
  neighbour's own checkpoint budget — exactly the cross-neighbour
  contamination D1 rejected reading real wall-clock time over in the first
  place, reintroduced one layer down. It would also make the anchor
  meaningless for every neighbour but whichever happens to drain first —
  not a partial degradation, a broken comparison for all the rest.
- **Anchor to each neighbour's own first `Dispatched` event (adopted)** —
  `t0` is set once, lazily, the first time `record_dispatch` is called for
  that neighbour's own `CheckpointTracker`; every subsequent dispatch/finish
  time is measured relative to it. This avoids counting real queue-wait
  against a neighbour's budget. **Known, accepted residual cost**: even
  from its own `t0`, a neighbour's `worker_slots` don't all fill
  simultaneously either — they trickle in one at a time as each real worker
  finishes the *previous* neighbour's tail instances and only then claims
  this neighbour's next token. So `basic_local_search` doesn't get
  `collect_one`'s fully clean "every worker's first dispatch at t≈0" bound —
  a smaller-scale version of the same ramp-up effect, inherited from
  whichever neighbour ran before it rather than from this neighbour's own
  queue. Not fixed now; flagged here for whoever revisits this.

**Cache hits and any dispatch this tracker never saw compose correctly with
the above without special-casing**, because they route through a genuinely
separate fallback that never touches `t0` at all: `record` with no matching
`record_dispatch` assigns to the currently-least-busy bucket and chains
`runtime` onto its existing relative-time total — exactly D10's old
arrival-based bookkeeping, operating in a purely relative frame that was
never tied to absolute time to begin with. A neighbour whose entire
evaluation is served from cache (no real dispatch ever happens, `t0` stays
`None`) still works correctly through this path alone.

## The `CheckpointTracker`

**Superseded by D11 (2026-09-13) — kept below as the historical record of
D6/D10, same treatment D4 got.** The struct and `record`/`score` shown here
are the pre-D11 (arrival-only) design; the real, current implementation adds
`t0`, `pending`, `assigned`, `record_dispatch`, `poll` and `refresh` per
D11's own section above — read the doc comments on `CheckpointTracker`
itself in `src/ils.rs` for the exact, current behaviour rather than this
snapshot. What's unchanged from here: `n_total`/`checkpoint_time`/
`cutoff_time`, the D6 solved-count definition, and `score()`'s contract.

One small piece of shared state, one per config being evaluated (i.e. one
per neighbour in `basic_local_search`'s `Vec`, one for the single config in
`collect_one`):

```rust
/// Replays `n_total` instances across `n_virtual` identical virtual
/// workers, fed real per-instance runtimes **as they actually arrive**
/// (D10, 2026-09-13) — list scheduling: each arriving result goes to
/// whichever virtual worker is currently least busy. Only needs `runtime`
/// (compared against `cutoff_time`, D6) — no `quality`, no
/// `run_obj`/`overall_obj` dependency, and no instance identity at all now
/// that position no longer matters.
struct CheckpointTracker {
    n_total: usize,
    checkpoint_time: f64,
    cutoff_time: f64,
    virtual_busy: Vec<f64>,
    /// Count of results virtually finished by `checkpoint_time` with
    /// `runtime < cutoff_time` — see D6 for why this is an exact "solved"
    /// count, not an approximation.
    solved_count: usize,
    /// How many real results this tracker has seen so far.
    n_seen: usize,
    /// True once every virtual worker's busy time exceeds `checkpoint_time`
    /// — mirrors the expericon stopping rule: assignment always goes to the
    /// least-busy worker. **Deliberately not** triggered by `n_seen ==
    /// n_total` — see the note on `score()` below.
    ready: bool,
}

impl CheckpointTracker {
    fn new(n_virtual: usize, checkpoint_time: f64, cutoff_time: f64, n_total: usize) -> Self { .. }

    /// Feed one more real result, in the order it actually arrived. No-op
    /// once `ready`.
    fn record(&mut self, runtime: f64) {
        if self.ready { return; }
        let w = self.virtual_busy.iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        let finish = self.virtual_busy[w] + runtime;
        if finish <= self.checkpoint_time && runtime < self.cutoff_time {
            self.solved_count += 1;
        }
        self.virtual_busy[w] = finish;
        self.n_seen += 1;
        if self.n_seen == self.n_total {
            // Every instance is real-accounted-for now. Return before
            // computing `ready` at all — see `score()`'s own note on why
            // this must never produce a usable answer at this point.
            return;
        }
        if self.virtual_busy.iter().all(|&t| t > self.checkpoint_time) {
            self.ready = true;
        }
    }

    /// `Some(n_total - solved_count)` only for a config that is **still in
    /// flight** when the virtual horizon matures (`ready` with `n_seen <
    /// n_total`) — a genuine early read; lower is better, per D6. `None` in
    /// every other case, including when `n_seen` reaches `n_total` *before*
    /// `ready` — that means the real evaluation already finished, so its
    /// exact score is already available through the normal
    /// `runtimes.len() == n_instances` path the caller checks anyway (see
    /// "Where this lives"), and the checkpoint has nothing to add.
    fn score(&self) -> Option<f64> {
        if !self.ready || self.n_seen >= self.n_total {
            return None;
        }
        Some((self.n_total - self.solved_count) as f64)
    }
}
```

**Implemented and tested**: `src/ils.rs`, right before the "Evaluation
helpers" section, with its own tests in the existing `mod tests` block
(arrival-order *dependence*, now demonstrated rather than disproved — see
D10 — plus the `runtime == cutoff_time` boundary case, the "finishes before
maturing → `None`" case, the rejected-mean/median regression case, and
three replay tests against
`tests/fixtures/checkpoint-replay/{auto,e-pre_casc_10,ram-2b65a8274485a1ea}.txt`
— real per-instance runtimes, fixed benchmark order, from the already-analyzed
`expericon` batch behind the diary's published reference counts, replayed in
that one recorded order). Wired into `run()`/`basic_local_search`/
`collect_one`; see "Where this lives" below.

**The replay tests confirm two things, one clean and one not**:
`e-pre_casc_10` and `ram-2b65a8274485a1ea` reproduce the diary's published
`38`/`47` exactly. `auto` does not — it gives `58`, not `55` — and this is
kept as the asserted, documented value rather than forced to match, because
tracing it down found a real bug **upstream, in the tool that produced this
specific dataset**, not in this port: 3 of 1999 `GaveUp` results carry their
real (sub-0.02s) elapsed time instead of the cutoff, because solverpy's
`Limiter.update()` only clamps `runtime` for statuses in `solver.timeouts`
(`ResourceOut`/`Timeout`), never checking `TPTP_FAILED` (`GaveUp`) — a PAR1
violation solverpy itself commits, unrelated to RamParILS's own wrapper
protocol (which requires PAR1 unconditionally and would not have this gap).
Filed as `gaveupfastruntime` in `solverpy`'s own `TODO.md`. This is exactly
the Risks section's "hard, direct dependency on wrapper PAR1 compliance"
warning, now with a measured, real-world instance rather than a hypothetical
one — worth re-reading that section with this in mind before deciding
whether `runtime < cutoff_time` ships as-is.

**Where this lives**: gated by D3 — the whole block below is skipped for a
round where `n_runs <= options.future_telling_cores`, no trackers built
at all. When active: `basic_local_search` gets a `Vec<CheckpointTracker>`
sized `n` (one per neighbour); `.record(result.instance_id, result.runtime)`
is called in the same spot the existing `runtimes[nid]`/`qualities[nid]`
vectors are updated (`quality` isn't needed at all under D6's redesign;
`instance_id` came back under D11 to correlate a `Completed` with its own
`Dispatched`, after D10 had dropped it). **Ordering matters at
this point**: the existing full-completion check (`runtimes[nid].len() ==
n_instances`) must be evaluated — and, if true, handled by the existing
accept/dominate path — *before* consulting `tracker.score()` for a possible
checkpoint rejection, never after. This isn't just belt-and-suspenders
around `score()`'s own guard: it's what guarantees an exact, already-complete
answer is never second-guessed by a heuristic one, even in the edge case
where the same incoming result both completes the neighbour *and* would
have matured its checkpoint. Order for the per-result handling in both
`collect_one` and the `basic_local_search` loop: append result →
adaptive-capping check (existing) → full-completion check (existing,
unchanged, always wins) → checkpoint check (new, only reached if the
neighbour is still incomplete after the first check). `collect_one` gets a
single `CheckpointTracker`, same treatment.

A config that reaches its own checkpoint — matured or not, rejected or not
— is logged once as `ils: future-telling-checkpoint`, separately from
`ils: future-telling-rejected`; both are temporary, kept deliberately
verbose while this feature is still being validated against real data.
`Scheduler::cancel_neighbor(batch_id, neighbor_id)` (`src/eval.rs`) is
called at the point a neighbour is rejected (both capping's and
future-telling's branches) — the fine-grained counterpart to `reset()`
that actually stops that one neighbour's own solver dispatch, added
2026-09-13 after RUN 06 showed rejected neighbours still accumulating
every real invocation regardless of being marked "done" on the consumer
side.

## New scenario options

Mirroring the existing `pruning`/`bound_multiplier`/`acceptance_tolerance`
pattern (`scenario.rs`). **`instance_shuffle`/`instance_shuffle_seed` are
not listed here** — they're documented under D5, since they're general
RamParILS instance-handling options this feature depends on rather than
`future_telling`-owned ones; everything below is genuinely specific to this
feature:

Renamed from an earlier `checkpoint_*` draft to `future_telling`/
`future_telling_*` (user's naming choice), and simplified so the feature has
no free-floating absolute-time constant of its own — it scales off
`cutoff_time`, which every scenario already has, rather than introducing a
second, unrelated time unit specific to whatever benchmark the `expericon`
finding happened to be measured on:

```yaml
future_telling: false              # opt-in master switch; off by default (see Risks)
future_telling_checkpoint: 1.0     # horizon as a multiple of cutoff_time — see below
future_telling_cores: null         # virtual worker count; null/unset = same as `cores`
future_telling_tolerance: 0.0      # relative margin, same shape as acceptance_tolerance
```

- **`future_telling_checkpoint`** replaces the earlier standalone
  `checkpoint_time`. Rather than a raw number of seconds (`5.0`, specific to
  the `eprover-ho`/`T5` cutoff structure the `expericon` finding was
  measured under, and meaningless as a default for a scenario with a very
  different `cutoff_time`), this is **which multiple of `cutoff_time` to use
  as the checkpoint horizon** — self-scaling to whatever the scenario's own
  per-instance cutoff already is, no second magic constant to pick per
  target algorithm. The actual horizon is derived once per `IlsOptions`
  construction: `future_telling_time = cutoff_time * future_telling_checkpoint`
  (an internal derived value — `CheckpointTracker` itself is unaware this
  came from a multiplier; it just takes a `checkpoint_time: f64`). Default
  `1.0` is deliberately conservative: it does **not** mean "check at the
  full cutoff instead of early" — maturity only needs the *least-busy*
  virtual worker's *cumulative assigned work* to pass the horizon, not any
  single instance's own runtime, so `n_runs >> future_telling_cores` (D3)
  alone can still let the schedule mature well before every instance is
  real-done, even at a full-cutoff horizon.

  **But `n_runs >> future_telling_cores` alone doesn't guarantee that** —
  there's a third factor D3's gate doesn't check: maturity roughly requires
  the *summed* real runtime of the instances processed so far to exceed
  `future_telling_cores * future_telling_time`. If typical per-instance
  runtimes are much smaller than `cutoff_time` (the common case — a cutoff
  is a worst-case bound, not a typical one), reaching that sum at the
  conservative `1.0` multiplier can require a very large `n_runs`, well
  beyond just clearing D3's threshold — so at the default, expect the
  checkpoint to mature rarely for a scenario whose instances mostly solve
  fast relative to `cutoff_time`, and expect that to be the common outcome
  the "config finished before maturing" case (D7) has to handle gracefully,
  not a rare corner. That's consistent with "conservative" — it errs toward
  inaction rather than reproducing the diary's `T5` accuracy — but it's
  worth stating plainly rather than implying the default reliably matures
  early whenever `n_runs` is merely large. A user chasing a sharper, earlier
  signal (closer to the diary's actual `T5` finding, which was a small
  fraction of `eprover-ho`'s cutoff) would lower this below `1.0`
  — that's a deliberate per-scenario tuning knob, not something to default
  aggressively without the validation pass having measured it.
- **`future_telling_cores`** replaces the earlier draft's
  `checkpoint_virtual_cores`, and drops its guessed absolute default
  (`256`) in favor of `null` (unset), which resolves to whatever `cores:`
  already resolved to for this run
  (`options.n_workers`) — no independent default to pick or justify, and a
  nice emergent property of D3 falls out of it for free: with the default,
  D3's gate (`n_runs > future_telling_cores`) becomes `n_runs > n_workers`,
  exactly the "config-by-config, no cross-neighbour contention" regime this
  conversation identified earlier as the one where a wall-clock read would
  even have been trustworthy in the first place — so the *default*
  configuration only activates future-telling in the regime this whole
  approach was designed around, without that boundary being hand-picked. A
  user chasing the diary's "more virtual cores, sharper signal" finding sets
  this explicitly higher than `cores:`; that's the override path, not the
  default.
- **`future_telling_tolerance`** unchanged in spirit from the earlier draft
  — see the worked example below.

Rejection rule, same shape as `accepted_within_tolerance` (`ils.rs:812`) but
inverted for a reject rather than an accept — reject only when the
challenger's checkpoint score (an *unsolved* count, D6 — lower is better,
same direction as every other score in this codebase, despite starting from
a solved-*count* signal where a bigger number would normally be better)
is worse than the incumbent's by more than the tolerance band. Deliberately
takes plain `f64` (holding a whole-number count), not `Option<f64>` — per
D8, the caller only ever calls this after unwrapping both sides, so
"reference missing" is handled once at the call site, not duplicated inside
this function:

```rust
fn future_telling_rejects(challenger_ckpt: f64, incumbent_ckpt: f64, options: &IlsOptions) -> bool {
    challenger_ckpt.is_finite()
        && incumbent_ckpt.is_finite()
        && challenger_ckpt > incumbent_ckpt + options.future_telling_tolerance * incumbent_ckpt.abs()
}
```

**Worked example**, incumbent checkpoint score `100` (i.e. 100 instances
still unsolved/pending out of this round's `n_runs`, D6), tolerance `0.1`:
the allowed band is `100 + 0.1 * |100| = 110` — the challenger is only
rejected once its own unsolved count exceeds `110`. A challenger at `105`
(5 *more* unsolved than the incumbent — a real but small degradation) stays
under `110`, so it does **not** fire — the 5-instance gap is smaller than
the 10-instance allowed margin, tolerated as noise. A challenger at `115`
(15 more unsolved) exceeds `110`, so it **does** fire. Note the direction: a
challenger at `95` (5 *fewer* unsolved, i.e. it has solved more) is
*better*, not worse — it can never trigger rejection regardless of
tolerance, since the check only fires upward from the incumbent's own
count. This is the same "lower is better" direction as `compute_score`
would give (D6's redesign keeps that convention even though the underlying
signal — solved count — is naturally a bigger-is-better one; `n_runs -
solved_count` is what flips it back to match everything else in the
codebase).

`future_telling_tolerance: 0.0` rejects on *any* checkpoint disagreement,
which given the diary's finding that disagreement clusters at close
final-score margins, is almost certainly too aggressive for a first cut —
see "Risks".

## Relationship to adaptive capping

| | adaptive capping (existing) | checkpoint reject (this proposal) |
|---|---|---|
| Trigger | running cost sum exceeds a budget derived from `incumbent_score` | simulated checkpoint score significantly worse than `incumbent_checkpoint` |
| Nature | mathematical proof (costs are non-negative and non-decreasing) | statistical heuristic (correlation measured on one dataset, not a guarantee) |
| Can it be wrong? | No — a capped score is a valid lower bound | **Yes** — can reject a config that would have gone on to win |
| Typically fires | later in a neighbour's evaluation, once enough real cost has accrued | potentially very early — as soon as enough *fast* results have streamed in to mature the virtual schedule |
| Instances covered | needs a partial *cost sum*, so needs some genuinely slow/expensive instances to accrue signal | needs enough *fast* results to mature the virtual horizon; a config whose first few real results are all slow may never mature at a small `checkpoint_time` |

They fire independently in the same loop; a neighbour can be pruned by
either without touching the other's bookkeeping. Neither should be read as
subsuming the other.

## Risks — read before setting a default

**This is the first heuristic (non-provable) prune in the codebase.**
Everything else that shortens an evaluation today (adaptive capping,
`Focused`'s fidelity growth) is either a proof or an explicit
lower-fidelity-and-say-so mechanism (`ConfigEvaluation::display`'s
`>score (n/total)` notation already exists precisely so a capped or
low-fidelity number is never silently read as a real one). A checkpoint
rejection has no such tell — a rejected neighbour is simply gone, counted
alongside real capping in whatever counter this feature adds, and the run
has no way to report "the good one might have been here." Recommend:

- Ship with `future_telling: false` by default.
- Log every checkpoint rejection distinctly from a capping rejection (a
  `future_telling::eval(rejected: bool)` counter parallel to the existing
  `counters` module, and its own debug line, e.g. `ils: future-telling-rejected
  neighbor=N ckpt=X ref=Y`), so a run summary can report the two separately
  and a suspicious result can be re-run with the feature off to check
  whether it changed the outcome.
- Do not default `future_telling_tolerance` to `0.0` in practice — pick a
  starting value only after the validation pass below gives an actual
  false-rejection rate to weigh it against, not from first principles.

**A config that finishes for real before its checkpoint matures is handled
by design, not left as an unhandled edge case** (see `CheckpointTracker`'s
`score()` and D3): such a config is never routed through the heuristic path
at all — it's caught by the existing exact accept/dominate logic first. This
was the point raised and fixed in this conversation before the design was
written down; called out here because it's the kind of thing easy to get
subtly wrong during implementation (an off-by-one in the check ordering
would silently let the heuristic override an exact answer) and worth a
dedicated test (validation item 5 below), not just a code review.

**The checkpoint metric has a hard, direct dependency on wrapper PAR1
compliance** (D6) — correcting an earlier, factually wrong version of this
risk item, which claimed only the quality objective's `UNKNOWN_QUALITY`
convention was guaranteed and runtime wasn't; checking
`docs/reference/protocol.md` shows PAR1 (`runtime = cutoff_time` on *any*
non-success line) is mandatory unconditionally, independent of `run_obj`.
D6's redesign leans on that guarantee directly: `solved_count` is computed
purely from `runtime < cutoff_time`, so a wrapper that violates PAR1 — reports
a fast, real runtime for a genuine failure — directly inflates `solved_count`
with false positives, corrupting the checkpoint silently. This isn't a new
exposure this feature invents: `docs/reference/protocol.md` documents a real
incident where a wrapper bug did exactly this (the 43%-error episode), and
the existing offline `ramparils db solved` export (`src/db.rs`'s
`SOLVED_STATUSES`) is *not* exposed to this failure mode the same way, since
it classifies by the raw `status` string rather than by runtime — worth
noting as a reason the two could disagree if a wrapper is buggy, and a good
sanity check to run if future-telling's rejections ever look suspicious
(compare against `ramparils db solved` for the same cache).

## Validation plan

Following this repo's own standing practice ("score any configuration
before trusting it," lifted from `expericon`'s equivalent finding): don't
trust this until it's checked against real data, twice.

1. **Done** — `checkpoint_tracker_arrival_order_is_irrelevant_to_the_final_count`,
   `checkpoint_tracker_runtime_equal_to_cutoff_is_not_solved`,
   `checkpoint_tracker_finishing_before_maturing_never_reports_ready` in
   `src/ils.rs`'s test module. Caught a real bug (see "The `CheckpointTracker`"
   above) — the arrival-order test is the reason this validation step was
   worth doing rather than skipping to the replay step directly.

   **Unit-level, before any integration**: feed `CheckpointTracker` a
   synthetic sequence of `(position, runtime)` pairs, fed in a deliberately
   scrambled arrival order, with a hand-computed expected virtual-worker
   assignment and `ready` transition, at a couple of `n_virtual` values
   including `n_virtual = 1` (degenerates to serial). The key property to
   assert: **the final `solved_count` is identical regardless of arrival
   order**, for a fixed set of `(position, runtime)` pairs — only how much
   of the run has to elapse before `ready` fires should differ. That's the
   whole point of buffering by fixed position rather than consuming in
   arrival order (D4); a test that only tries one arrival order can't catch
   a cursor/buffering bug that happens to work for the common case. Include
   at least one case with `runtime == cutoff_time` exactly, asserting it is
   *not* counted as solved (the `<`, not `<=`, in D6 matters).
2. **Done** — `checkpoint_tracker_rejected_design_would_have_inverted_the_comparison`
   in the same test module, confirms the actual design does not reproduce
   the inversion.

   **Rejected: `compute_score` (mean/median of runtime/quality)** — recorded
   here as the concrete failure case D6 refers to, not a design still under
   consideration. Construct two synthetic configs at the same checkpoint
   maturity: config A has 3 completions, all with excellent runtime; config
   B has 500 completions, mostly decent, none as good as A's 3. Under the
   original mean/median design, A's checkpoint score beats B's, despite B
   having done vastly more confirmed-good work — the opposite of what a
   volume-sensitive, `expericon`-style signal should say. Worth keeping as
   a regression-shaped test (`n_runs - solved_count` should *not* reproduce
   this inversion) rather than only as a paper argument.
3. **Done, with a real finding** — `checkpoint_tracker_replay_e_pre_casc_10_matches_the_diary_exactly`
   and `checkpoint_tracker_replay_ram_2b65a827_matches_the_diary_exactly` pass
   exactly (`38`, `47`); `checkpoint_tracker_replay_auto_reveals_a_real_par1_violation_upstream`
   asserts `58`, not the diary's `55`, and documents why in its own doc
   comment — see "The `CheckpointTracker`" above for the root cause
   (solverpy's own `Limiter` bug, filed upstream) and why `58` is the correct
   thing to assert, not a fixture to doctor into agreement. Fixtures are
   `tests/fixtures/checkpoint-replay/*.txt` — numbers only, no instance
   paths, no host/experiment identifiers (this repo is public).

   **Replay validation against `expericon`'s own already-collected data**:
   take one of the `subtrainNNN`/`train2k` per-instance result sets already
   sitting in
   `work26/expericon/ramparils-eprover/servers/.../solverpy_db/results/`
   (real `runtime` + real SZS/SMT status per instance, for named strategies
   including `auto`/`e-pre_casc_10`/`ram-2b65a8274485a1ea`), feed each
   strategy's real runtimes through `CheckpointTracker` at `n_virtual = 64`,
   `checkpoint_time = 5.0`, and that batch's real `cutoff_time`, fed in a
   few different *simulated arrival* orders (shuffled, standing in for real
   out-of-order completion) while keeping the *fixed position* argument tied
   to each result's true benchmark-order index. **Two things to check, not
   one**: (a) `solved_count` is identical across every arrival order tried
   (same buffering-correctness property as item 1, now on real, messy data
   instead of a hand-built example); (b) `solved_count` matches the diary's
   own already-published, already-sanity-checked exact numbers — `55`/`38`/
   `47` for those three strategies at real `N=64` — directly, no test-only
   instrumentation needed, since `solved_count` *is* the diary's metric now
   (D6), not a derived approximation of it. A mismatch here means either a
   bug in the port or a real wrapper PAR1 violation somewhere in that
   dataset (see "Risks") — worth distinguishing which before writing it off
   as a rounding difference.
4. **Done** — `shuffle_instances_is_deterministic_given_a_fixed_seed`,
   `shuffle_instances_differs_across_seeds`, `shuffle_instances_is_a_pure_reordering`,
   `instance_shuffle_never_changes_which_id_a_path_resolves_to` (via a real
   `Cache`) and `shuffle_warning_fires_only_for_future_telling_without_shuffle`
   (all four combinations) in `src/ils.rs`/`src/scenario.rs`'s test modules.

   **Instance-shuffle unit tests** (D5, independent of `CheckpointTracker`
   itself): (a) determinism — the same `instance_shuffle_seed` produces the
   same shuffled order across repeated calls, and a different seed produces
   a different one; (b) instance-ID independence — shuffling settings never
   change which `instance_id` a given path resolves to, only the order of
   the already-ID-assigned `(i64, String)` pairs `run()` operates over
   afterward (construct a small cache, load instances, assert IDs are
   identical whether or not `instance_shuffle` is set); (c) the
   `future_telling: true` + `instance_shuffle: false` warning fires exactly
   on that combination and on no other (`false`/`false`, `false`/`true`,
   `true`/`true` all silent).
5. **Done, with a revised approach** — see "What the integration tests
   actually show" (below the staged checklist) for what changed and why.
   Four tests in `src/ils.rs`'s test module:
   `future_telling_stops_evaluating_a_bad_config_before_full_completion`,
   `future_telling_never_rejects_a_config_whose_checkpoint_never_matures`,
   `future_telling_gate_blocks_rejection_when_n_runs_does_not_exceed_virtual_cores`,
   `future_telling_never_lets_a_rejected_neighbour_win_a_round`.

   **Integration test**: an end-to-end `ils::run()` test (small synthetic
   `ParamSpace`, a fake `algo` shell script whose per-instance runtime is
   scripted per instance name) asserting that a neighbour engineered to be
   obviously worse gets rejected with fewer than `n_instances` real solver
   invocations, and that a neighbour engineered to be obviously better is
   never rejected — the two-sided check, since a test that only checks "bad
   things get pruned" can't catch a tolerance set so tight it also prunes
   good ones. Two more cases belong in this same suite, both direct
   consequences of D3 and the full-completion-first ordering:
   - a scenario with `n_runs <= future_telling_cores` never builds or
     consults a `CheckpointTracker` at all (D3) — assert no checkpoint debug
     line or counter increment fires, not just "no rejection happened,"
     since the latter could pass for the wrong reason;
   - a neighbour engineered to finish all its instances *fast*, well before
     `checkpoint_time`, is accepted or rejected purely by the normal
     accept/dominate path, never by a checkpoint verdict — assert via the
     debug log that its outcome is attributed to the ordinary path, not
     logged as a checkpoint rejection, covering the exact edge case this
     conversation identified in `CheckpointTracker::score()`.
6. **Real tuning comparison**: once integrated and default-off, run the
   same scenario twice on the same host — `future_telling: false` vs
   `true` — and score both final configurations with the same solverpy
   batch, same as any other RamPaRILS output. **Hold `instance_shuffle` (and
   its seed) fixed and identical across both runs** — it's an independent
   setting from `future_telling` (D5), and letting it vary too would
   confound the comparison this step exists to isolate. Only promote the
   default to `true` after that comparison, on a real scenario, not a
   synthetic one.

## Open questions for the user

- **Resolved by D11 (2026-09-13): assign virtual busy-time at dispatch, not
  at arrival.** See D11 above for the design (`eval.rs`'s result channel
  becomes `Sender<SchedulerEvent>` carrying `Dispatched`/`Completed`
  variants instead of a bare `TaskResult`, `record_dispatch()` on
  `CheckpointTracker`, and the `future_telling_cores <= cores` requirement
  for a clean `checkpoint_time + ε` bound) and `ramparils-eprover/DIARY.md`,
  2026-09-13, for the RUN 06 measurement that raised it. **Not yet
  implemented in code** — D11 is the design only; see the staged
  implementation checklist.
- **`future_telling_checkpoint` default**: `1.0` (checkpoint horizon equal to
  the scenario's own `cutoff_time`) is chosen to be self-scaling and
  conservative rather than to reproduce the diary's exact numbers — the
  `expericon` finding's `T=5s` was a small fraction of `eprover-ho`'s own
  cutoff, not "5 seconds" as an absolute constant meaningful across
  scenarios. Item 3's replay validation (done) only checked the diary's own
  exact settings (`checkpoint_time = 5.0` = `cutoff_time`, no multiplier
  involved); still open: should a follow-up sweep this multiplier (e.g. 1.0
  vs 0.2 vs 0.05 of `cutoff_time`) against the same replay fixtures to check
  whether a smaller default gets closer to the diary's actual accuracy
  numbers, or is `1.0` — conservative, but slower to mature, per the
  derivation in "New scenario options" — the safer place to start given
  this is the first heuristic prune in the codebase (see "Risks")?
- **`future_telling_cores` default** (`null` → `cores`) removes the earlier
  "which absolute number" question, but not every question: a user chasing
  the diary's "more virtual cores, sharper signal" finding has to override
  it explicitly, with no guidance yet on how much higher than `cores:` is
  worth it for a given scenario. Item 3 used `n_virtual = 64` only (matching
  the diary's own reference measurement); still open: should a follow-up
  sweep a few multiples of `cores` (2x, 4x, ...) against the same replay
  fixtures to give that user a
  starting point, the way the diary's own sweep did for its dataset? Worth
  restating the D3 coupling once more since it applies here too: overriding
  this upward for a sharper signal also raises the `n_runs` bar for when the
  feature activates at all, so a `Focused` run pushed to override it high
  should be checked against that same real `Focused` scenario's
  typical `n_total` and `fidelity_step`, not decided from the accuracy sweep
  alone.
- **Resolved: `home_base_checkpoint` (D9) was built now, not deferred.** It
  turned out just as cheap as expected — read straight off `last_lm_eval`'s
  new `checkpoint` field at every site that already updates `last_lm_eval`,
  no separate tracker or call-site threading needed. Still genuinely idle
  (silenced with `let _ = home_base_checkpoint;` at the end of `run()`) —
  the "concrete second use" this question asked about hasn't materialized
  yet, so it remains ready-but-unread rather than proof the choice was
  right.
- **Multiple checkpoint times?** This design is deliberately single-`T`
  (matching "T5 checkpoint seems to be best" from the previous turn). The
  diary's slider tooling showed accuracy is *not* monotonic in checkpoint
  time (real-cutoff synchronization artifacts at certain `T`s) — if a future
  need arises to hedge across a couple of `T`s rather than commit to one,
  that's a bigger change (one `CheckpointTracker` per `T`) worth scoping
  separately rather than building speculatively now.

## Staged implementation checklist

- [x] `CheckpointTracker` type + unit tests (validation item 1), including
      the `runtime == cutoff_time` boundary case and the rejected-design
      regression case (validation item 2) — found and fixed one real bug
      along the way (the `ready`-per-batch-not-per-position issue, see
      "The `CheckpointTracker`")
- [x] Replay validation against existing `expericon` data (validation
      item 3) — direct `solved_count` comparison against the diary's
      already-published `55`/`38`/`47`; two of three matched exactly, the
      third (`auto`, `58` not `55`) traced to a real upstream bug (filed as
      `gaveupfastruntime` in `solverpy`'s `TODO.md`), not a bug in this port
- [x] `instance_shuffle`/`instance_shuffle_seed` scenario fields (D5),
      top-level, not `future_telling`-prefixed; shuffle built in `run()`
      right after the scheduler/rng are constructed, via a standalone
      `shuffle_instances()` helper, **after** `cache.load_instances()` has
      already assigned every `instance_id` — never reorder before ID
      assignment
- [x] Debug line recording that the shuffle ran and with what seed (D5) —
      `ils: instance_shuffle applied to N instances (seed=S)`
- [x] Startup warning for `future_telling: true` + `instance_shuffle:
      false` (D5) — a plain, unit-tested decision function
      (`future_telling_needs_shuffle_warning`) plus an `eprintln!` in
      `Scenario::ils_options`, not a hard error
- [x] Instance-shuffle unit tests (validation item 4) — determinism given a
      fixed seed, a different seed giving a different order, the shuffle
      being a pure reordering (never a resample), instance-ID independence
      against a real `Cache`, and all four `future_telling`/`instance_shuffle`
      combinations for the warning
- [x] Scenario fields (`future_telling`, `future_telling_checkpoint`,
      `future_telling_cores`, `future_telling_tolerance`) + `IlsOptions`
      plumbing, mirroring `pruning`/`bound_multiplier`; `future_telling_time`
      derived inline from the `cutoff_time` each call site already has
      (correct across iterative-deepening's per-phase cutoffs too, not just
      the scenario's own); `future_telling_cores: null` resolved to
      `n_workers` in `Scenario::ils_options`, same point `cores: 0` already
      resolves to "all available"
- [x] `n_runs > future_telling_cores` gate (D3) — `future_telling_active()`,
      checked once per round before any `CheckpointTracker` is constructed,
      unit-tested directly at the boundary (`==` must not open the gate)
- [x] Wire `CheckpointTracker` into `collect_one`, with the full-completion
      check strictly before the checkpoint check (D3/`score()` ordering)
- [x] Wire `CheckpointTracker` into `basic_local_search`'s neighbour loop,
      same ordering — one tracker per neighbour
- [x] Dropped fixed instance order for arrival order (D10, 2026-09-13),
      after RUN 06 showed the fixed-order design stalling on stragglers
      while newer data sat unused; `Scheduler::cancel_neighbor` added the
      same day so a rejected neighbour's own dispatch actually stops
- [x] `incumbent_checkpoint` and `home_base_checkpoint` threaded through
      `run()` at every incumbent/home-base update site — **reassigned
      unconditionally at each one, including to `None`** (D7), read off a
      new `checkpoint: Option<f64>` field on `ConfigEvaluation` itself rather
      than as separately-threaded state. `home_base_checkpoint` is recorded
      but still unread (D9's open question resolved as "build it now, it was
      nearly free") — silenced with a `let _ = home_base_checkpoint;` and a
      comment at the point it would otherwise warn as dead.
- [x] Distinct counter + debug line for checkpoint rejections (Risks) —
      `counters::FUTURE_TELLING_REJECTED`, reported as
      `future_telling_rejected=N` in the end-of-run `ils: summary` line,
      alongside (not instead of) the existing generic `capped` bucket
- [x] Integration test (validation item 5) — see "What the integration tests
      actually show" below; the exact framing in this checklist's earlier
      draft ("fewer than n_instances real solver invocations") turned out not
      to be a reliable thing to assert against this scheduler's own
      concurrency model, and the tests were reshaped around a property that
      actually is deterministic and provable
- [x] `docs/` update — `docs/usage/cli.md` and `docs/usage/python.md` gained
      `instance_shuffle`/`future_telling` field tables and a new
      "🔮 Future-telling" section; `docs/reference/algorithm.md` gained a
      "Future-telling" section (linked from the glossary) and an updated
      summary-line example; `docs/reference/glossary.md` gained
      *Checkpoint*, *Future-telling* and *Instance shuffle* entries and
      fixed two now-stale claims ("instances are not shuffled") in the
      existing *Fidelity* entry and both usage pages
- [x] `CHANGELOG.md` entry under `## [Unreleased]` — two entries, one for
      `future_telling` and one calling out `instance_shuffle`'s default
      change explicitly as its own bullet, not folded into the feature's
- [ ] Real tuning comparison before flipping any default (validation
      item 6), holding `instance_shuffle` fixed across both sides — still
      not done; explicitly out of scope for this implementation pass
- [x] **D11 — dispatch-time-based `CheckpointTracker`.** `eval.rs`'s event
      channel is now `Sender<SchedulerEvent>`/`Receiver<SchedulerEvent>`
      (`Dispatched(DispatchEvent) | Completed(TaskResult)`, method renamed
      `results()` → `events()`), emitting `Dispatched` right after
      `next_index.fetch_add`, before `run_solver_inner` — one channel, no
      `select!`. `CheckpointTracker` gained `t0` (per-neighbour relative-time
      origin, seeded from its own first `record_dispatch`, never from
      `submit()` or a cache hit — see D11's "Resolved during implementation"
      note on why), `pending`/`assigned` (instance_id-correlated, so a
      `record_dispatch` and its matching `record` settle the same virtual
      bucket), `record_dispatch`, `poll` (real-time re-check when no event
      arrives to trigger `refresh` on its own) and `refresh`. A completion
      with no matching dispatch (a cache hit, or more concurrently in-flight
      real dispatches than `future_telling_cores` — the `<` case) falls back
      to D10's original least-busy-bucket assignment unchanged. Both
      `collect_one` and `basic_local_search`'s consumer loops now match on
      the enum and call `tracker.poll(crate::t())` on every recv-timeout,
      not just on a new event. All prior `CheckpointTracker` unit/replay
      tests pass unmodified in behaviour (just an added `instance_id`
      argument, since a bare `record()` with no prior `record_dispatch`
      reproduces D10 exactly); one integration test's fixture
      (`future_telling_never_rejects_a_config_whose_checkpoint_never_matures`)
      needed updating — it used `n_workers=1` with `future_telling_cores=4`,
      an oversubscribed setup where D11's own documented degraded bound
      (`ceil(4/1)*checkpoint_time`) legitimately matures the checkpoint
      where D10's fully-virtual model never would have; fixed by matching
      `n_workers` to `future_telling_cores` and raising the checkpoint
      multiplier, per D11's own "no oversubscription" requirement for the
      clean bound. `cargo test`/`clippy`/`fmt --check` all clean (the
      `fmt --check` diffs present are pre-existing rustfmt-version drift,
      confirmed via `git stash`, not introduced here). **Not done**: the
      replay-fixture re-check against real RUN 06-shaped data (still
      simulated/unit-level so far, not re-run against real solver logs), and
      D11's own flagged residual (`basic_local_search`'s per-neighbour
      ramp-up skew, inherited from whichever neighbour drained before it) is
      accepted, not fixed.

## What the integration tests actually show

Validation item 5's original wording ("a neighbour engineered to be
obviously worse gets rejected with fewer than `n_instances` real solver
invocations") assumed marking a neighbour `done[nid] = true` on the ILS
consumer side also stops its solver dispatch. **At the time this was
written, it didn't** — `Scheduler`'s worker threads kept pulling from a
rejected neighbour's own `WorkBatch` via its shared `next_index` atomic
regardless of what the consumer decided, until a `scheduler.reset()` (fired
only on an accept or at round end) actually cancelled it. Confirmed for
real, not just in a test, on `ramparils-eprover` RUN 06 (2026-09-13): two
future-telling-rejected neighbours each still accumulated all 2000 real
solver invocations, confirmed directly against the run's own `dbcache`
(`SELECT strategy_hash, COUNT(*) FROM results GROUP BY strategy_hash`).

**Fixed the same day**: `Scheduler::cancel_neighbor(batch_id, neighbor_id)`
(`src/eval.rs`) is the fine-grained counterpart to `reset()` — it terminates
any currently-running process for that one `(batch_id, neighbor_id)` and
marks the pair cancelled so workers stop picking up its remaining queued
instances, without touching any other neighbour still legitimately in
flight. Both `basic_local_search`'s capping-reject and checkpoint-reject
branches call it now. Tested directly at the scheduler level
(`cancel_neighbor_terminates_only_that_neighbors_solver`, `src/eval.rs`) —
two neighbours each on their own worker, cancelling one kills its process
while the other's keeps running untouched. The paragraphs below (and the
"real solver invocations" framing they moved away from) describe the state
*before* this fix; kept as the record of how the gap was found, not
because it's still open.

What's actually tested instead, all in `src/ils.rs`'s test module:

- `future_telling_stops_evaluating_a_bad_config_before_full_completion` and
  `future_telling_never_rejects_a_config_whose_checkpoint_never_matures` —
  the real, reliable contract: a checkpoint-rejected single-config evaluation
  (via `evaluate_config_outcome`/`collect_one` directly) reports
  `complete: false` with `n_done < n_instances`, the same "stop watching it,
  count it as done" outcome adaptive capping already produces (D8) — and an
  obviously-good config, whose tracker structurally never matures at the
  chosen horizon, always runs to real completion regardless of how strict
  the reference is. Both use a single real worker so real arrival order is
  deterministic (no other config competing for threads), which is what makes
  the exact point of rejection reproducible instead of a race.
- `future_telling_gate_blocks_rejection_when_n_runs_does_not_exceed_virtual_cores`
  — D3's gate, checked structurally (a deliberately unsatisfiable reference
  still can't produce a rejection below the gate) rather than by "no
  rejection happened" alone, which could pass for the wrong reason.
- `future_telling_never_lets_a_rejected_neighbour_win_a_round` — a
  full-pipeline wiring/non-regression check at the `basic_local_search`
  level: with both a checkpoint-eligible `bad` neighbour and a never-matures
  `good` one in the same round, the returned local optimum is still the good
  one. Its own doc comment is explicit that this doesn't isolate the
  feature's causal contribution (`bad`'s real, completed score can't
  dominate the baseline either way) — it exists to catch a wiring mistake
  that corrupts the ordinary accept path, not to re-prove the point the two
  tests above already prove more directly.
