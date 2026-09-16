# RamParILS — Open TODO

Legend: 🏗 scaffolded (types/stubs exist, logic missing) · ⬜ not started

Finished implementation work and established conventions live in `DONE.md`,
split out 2026-08-21 so this file stays a list of what's actually open.

---

# Proposed — provenance and inert-parameter detection (2026-08-12)

Four items from the `expericon/ramparils-primo` experiment, which has now hit
the same class of failure three times: **a parameter that parses, reaches the
solver's command line, and changes nothing.** It never errors. The symptom is
whole neighbourhoods scoring identically, which reads as a plateau rather than
as a defect, so the search spends its budget without anyone noticing.

The three occurrences so far:

1. primo's four `--lra-*-max-row-size` / `-max-fanout` options were written by
   the CLI and read by nobody until primo `0d73973` (2026-08-10). Runs 5 and 6
   carried them as four dead dimensions for 24 hours of tuning.
2. Any parameter absent from the wrapper's `VALUE_OPTIONS` dict is silently
   dropped. `air-03` held a pre-`819a582` wrapper for a day while its host
   survey reported a fully up-to-date `ramparils --version`.
3. `--cadical-option seed` is inert under primo's external propagator: seeds 1,
   7 and 99 give byte-identical decisions (8420) and conflicts (726).
   Documented in primo `b3c4188`.

`scripts/params-info.py` in the experiment workspace catches the *structural*
version of this (a parameter inactive at the default, or reachable in too
little of the space). It cannot catch any of the three above, because in each
case the parameter is active by every structural test.

Note items 1 and 2 are different from item 4 below, and only 1–3 deserve the
name "dead". Item 4 is about a parameter that genuinely changes behaviour by an
amount the objective cannot resolve — a property of the experiment (budget,
cutoff, instance set), not of the solver.

**✅ Wrapper `--version` protocol — done, see `DONE.md`.**

## ⬜ Wrapper parameter-contract check at startup

Add a wrapper query — `--list-parameters`, printing the parameter names the
wrapper knows how to pass — and have `ramparils` compare it against the
parameter file at startup, refusing to start when the file names a parameter
the wrapper would drop.

The cheapest of the four: **no solver calls at all**, pure string comparison,
and it catches occurrence 2 above completely. Should be an error rather than a
warning — a silently reduced search space produces a plausible-looking result,
which is worse than a crash.

## ⬜ Behavioural probe for structurally dead parameters

At startup, for each parameter, run the initial configuration against one
variant per extreme domain value on a small sample of fast instances, and
compare **the solver's own statistics**, not the runtime. Byte-identical
statistics ⇒ that parameter changed nothing, so report it and let the user
decide whether to continue.

Runtime cannot be the signal — wall-clock differs between two runs of an
identical command line, so a timing comparison can neither confirm nor refute
deadness. This needs the wrapper to return a statistics digest alongside the
runtime, which pairs naturally with the `--version` item above (both are
wrapper-contract extensions).

Catches occurrences 1 and 3, which nothing else can. Cost is roughly
`|params| × 2 × |sample|` solver calls: for an 11-parameter space and 8 fast
instances, ~176 calls, a couple of minutes on 60 workers.

**Building blocks now exist, the startup probe itself does not.** The
statistics-digest half is built: `solverpy`'s `runhash()` (a SHA-256 over a
sorted-key subset of the solver's own result counters, XOR-combinable across
instances) is wired through both `primo.py` and `eprover.py`, `ramparils`
stores it per result (`cache.rs`'s nullable `results.runhash` column,
`TaskResult`/`ConfigEvaluation` carry it, `ils.rs` XORs it across a descent's
neighbours and logs it beside each incumbent), and `ramparils db runhashes`
exports a per-strategy `ram-<hash> <runhash> <n>` line so two configurations
that produced byte-identical solver behaviour can be spotted after the fact
(`work26/expericon/scripts/group-runhashes.py` groups them). What is still
missing is exactly the "at startup, run one variant per extreme domain value
and compare automatically" loop this item describes — today the comparison is
a manual post-hoc `ramparils db` + script pass over a completed run, not a
preflight gate.

## ⬜ Noise-relative inertness report at end of run

Report, per parameter: how many times a neighbour differing only in it was
evaluated, the distribution of the resulting score deltas, the run's own noise
floor, and a verdict.

`ramparils` already has almost all of this — it evaluates every neighbour of
every descent, so the per-parameter delta distributions are free. The missing
piece is **a noise floor measured by the run itself**, obtained by periodically
re-evaluating the incumbent under a fresh cache key (a self-`dup`). At
neighbourhood 16 doing that once per descent is ~6% overhead.

The floor must be measured, not assumed, and it is not symmetric: in the
experiment's own data, re-measuring an identical configuration made 55.6% of
instances "slower" with a median ratio of 1.0075 — so the null is a biased
55.6%, not 50%. With first-improvement acceptance, a biased null means the
search accepts noise at better than chance in one direction.

Motivating evidence: three independent 12–24 h runs of the same 11-parameter
space, from two different starting configurations on three machines, converged
on the same two parameters that matter — and disagreed completely on three
others, picking three different values spanning the whole domain, while those
three absorbed the most home-base movement of anything in the space. All three
of those parameters are *live* (their extreme values move runtime by 2–13%),
but flat in the region the search explores. The runs had no way to know that,
and spent ~31% of every descent there.

## ⬜ Post-hoc minimisation pass after a tuning run

After the search finishes, greedily minimise the final configuration: repeatedly
try reverting each remaining departure-from-default to its default, drop the one
whose removal costs least, and stop when every remaining removal costs more than
the noise floor. Report both the raw final and the minimised configuration.

**Why: BasicILS compares whole configurations and has no mechanism to drop a
component once accepted.** It can add an option that helps in the presence of a
second one, then keep it forever after the second is replaced. The experiment
measured how bad this gets — an exhaustive 2^9 ablation of run 6's nine-option
final found that **8 of the 9 departures from primo's defaults were unhelpful**,
and the best 3-option subset scored **+10 instances** against the run's own
final at T30 (noise floor ~5). Run 6 had already shed 9 → 5 options on its own
over 12 hours; the ablation took it 5 → 3 in one pass.

A shorter configuration is not merely tidier. It is the difference between a
result a reader can act on and a nine-flag incantation, and every option kept
without evidence is one more thing that can interact badly with the next solver
release.

**Cost, measured rather than estimated.** Exhaustive 2^k is affordable only for
small k — the 2^9 = 512 enumeration cost 715 core-hours on the full
1,753-instance benchmark. Two independent reductions apply:

- **Greedy backward elimination is O(k²), not O(2^k)**: k(k+1)/2 = 45
  evaluations for k = 9 against 512, ~11x fewer.
- **The tuning subset is a sufficient proxy for this particular question.**
  Re-scoring all 512 on the 473-instance tuning subset instead of the full
  1,753 selects `b000010101` — the configuration actually adopted as best
  known — at a cost of **+1 instance** on the full set, with Spearman rank
  correlation 0.896 across all 512 and the subset winner ranking 3 of 512.
  That is 715 → 175 core-hours, 4.1x.

Together: **~15 core-hours, or about 15 minutes on 60 workers**, to minimise a
nine-option configuration — against the 12 hours the tuning run itself cost. It
should simply be the tail of every run.

Three design notes:

- **The tolerance must come from a measured noise floor**, not a constant. This
  shares its mechanism with the inertness report above: without a floor, greedy
  elimination will either stop immediately or strip options that matter.
- **Reuse the cache.** Many minimisation candidates were already evaluated
  during the search, and the cache is keyed by `hash_config(active_config)`, so
  a large fraction of the 45 evaluations should be free.
- **Removal is not the only move for non-binary parameters.** The 2^9 ablation
  tested each option only as present (the run's value) versus absent (primo's
  default), and five of the nine were not boolean — so values between the two
  were never tried. A minimisation pass that only removes inherits that blind
  spot; either sweep the domain of each retained parameter afterwards, or state
  plainly that the output is minimal-by-removal rather than optimal.

Caveat on the subset result: it holds because this ablation ranks nested subsets
of one configuration, where the signal is dominated by two large main effects
that the subset preserves. The 473 are **not** a uniform miniature of the
benchmark — they carry 8 of 488 `meti-tarski` and 103 of 144 `sc` — so the same
substitution should not be assumed for a question whose answer lives in a family
the subset under-samples.

---

# Proposed — random configuration sampling (2026-08-20)

Items raised while checking whether `approach: random` is usable as a
random-restart baseline for the `expericon/ramparils-primo-cont` work. It is
implemented and it works — a 45 s smoke run over 21 instances
(`work26/ramparils/primo/scenario-random.yml`, 2026-08-20) logged 17
`ils: random restart` lines, 18 `ils: new home base` lines and 0 `ils: restart:`
lines, which is exactly the intended shape. The first four items are about
`random_config` (`src/ils.rs:1261`), whose sampler is uniform over the **full
cross-product** and rejection-tests the **unprojected** draw; the last is about
what the run header claims.

The function today, in full:

```rust
fn random_config(space: &ParamSpace, rng: &mut impl Rng) -> Config {
    loop {
        let cfg: Config = space
            .params
            .iter()
            .map(|p| (p.name.clone(), p.domain[rng.gen_range(0..p.domain.len())].clone()))
            .collect();
        if !space.is_forbidden(&cfg) {
            return cfg;
        }
    }
}
```

Line references below are against **`ea6bed1`** (v0.2.0 plus the docs sweep);
the pre-v0.2.0 rustfmt pass moved every number in this file's neighbourhood
without changing behaviour, so re-resolve by symbol if they drift again.

Four call sites, so anything below affects more than `approach: random`: the
initial configuration when the scenario supplies none (`ils.rs:250`),
`random_probes` (`275`), the per-round draw under `Approach::Random` (`352`),
and `restart_target: random` (`480`).

**What is already correct, and must stay correct.** Conditionals *are* honoured
everywhere downstream: `evaluate_config` projects through
`active_config(config, space)` before `hash_config` and before dispatch
(`ils.rs:799-800`), so inactive values never reach the wrapper and never enter
the cache key; `neighbourhood` iterates active params only (`ils.rs:617`), so
BLS and `perturbation` cannot move an inactive parameter. None of the items
below should change any of that — the shadow values in the returned `Config`
are what let a draw be projected consistently later.

## ⬜ Bound the rejection loop and fall back to constructive sampling

The `loop` has no attempt limit. Rejection sampling costs O(1/p) draws where p
is the legal fraction of the space, so a space where forbidden clauses exclude
almost everything **spins forever with no output and no error** — the process
looks like it is working.

This is not hypothetical: the user hit exactly this failure with the original
Ruby ParamILS on a large space with a large forbidden set. RamParILS inherits
the shape. It has not been observed here, and it cannot be in the primo spaces
— `params-primo-v{1,2,3,4}.txt` and both v3 companions contain **zero**
forbidden clauses, because that record deliberately encodes dependencies as
conditionals instead. Any user with a genuinely constrained space is exposed.

Fix in two parts:

- **Cap the attempts** (a few thousand), then fail with a diagnostic naming the
  measured rejection rate and pointing at the forbidden clauses, rather than
  hanging. An error the user can read beats a silent hang in every case.
- **Fall back to a constructive draw** before erroring: assign parameters in
  dependency order, sampling each from the values that keep every
  fully-assigned clause satisfiable, and backtrack on a dead end. That turns
  "p is tiny" from fatal into merely slower, and it is the standard fix.

Note `perturbation` and `neighbourhood` do not share the defect — they
enumerate candidates and filter, so an over-constrained point yields an empty
neighbourhood and `perturbation` simply breaks out (`ils.rs:644`).

**✅ Test forbidden clauses against the active projection — done, see `DONE.md`.**

## ⬜ Decide, and document, what `random` is uniform over

Drawing every parameter independently and projecting afterwards means an active
configuration is sampled with probability proportional to the size of the
sub-tree its guard switches off. In `expericon`'s
`params-primo-v3-prop.txt`, where `lra_bidirectional_row_propagation` guards two
4-valued children:

| active configuration | P(drawn) | uniform would be |
|---|---:|---:|
| guard `false`, children inactive | **1/2** | 1/17 |
| each of the 16 guard-`true` cells | 1/32 | 1/17 |

**8.5x over-representation of the guarded-off corner**, which is usually where
the defaults sit — so a random-restart baseline flatters itself on any space
whose defaults are good. `params-primo-v3.txt`, with one conditional, gives a
mild 1.5x; the effect scales with the number and fan-out of guards.

This is faithful to ParamILS, whose `init_random()` also draws each parameter
independently, so it is not a deviation from the reference — which is an
argument for documenting it rather than silently changing it. Either:

- keep the current behaviour and **say so** in `docs/reference/algorithm.md`
  beside the `approach: random` paragraph, so a reported baseline can be read
  correctly; or
- add an opt-in uniform-over-active-configurations sampler (draw the guards
  first, then only the parameters they activate, weighting so each distinct
  active configuration is equiprobable) and let the scenario choose.

Whichever is chosen, the sampler's distribution belongs in the docs: a
random-restart arm is a *measurement instrument*, and an instrument with an
undocumented 8.5x bias produces numbers nobody can interpret.

## ⬜ Test coverage for all of the above

`random_config` has one test, `random_config_not_forbidden` (`ils.rs:1668`),
over `forbidden_space()` (`1292`) — two parameters, no conditionals.
`conditional_space()` (`1303`) exists but is used only by `ils.rs:1365` and
`1379`, and **no test constructs `Approach::Random` at all**. Worth adding:

- a space where forbidden clauses exclude nearly everything, asserting that
  `random_config` **terminates** (with an error or a constructive draw) rather
  than hanging — this is the regression test for the first item, and it needs a
  timeout guard so a failure fails rather than hangs CI;
- a conditional space with a clause naming an inactive parameter, asserting the
  draw is accepted;
- a distribution test over a small guarded space, asserting whichever
  uniformity the third item settles on;
- one end-to-end `Approach::Random` run asserting the acceptance criterion is
  skipped and each round starts from a fresh draw.

## ⬜ Mark or suppress options the active approach ignores

The run header (`src/main.rs:201-226`) prints `fidelity:`, `perturb:` and
`restart:` unconditionally, so a `random` run reports settings that provably do
nothing. From the smoke run above, whose scenario sets none of them:

```
[    0.02s] approach:   random
[    0.02s] fidelity:   initial=1 step=1
[    0.02s] perturb:    strength=4 restart_strength=8
[    0.02s] restart:    p=0 failures=0 target=incumbent tolerance=0 probes=0
```

What is actually inert, by approach:

| option | `focused` | `basic` | `random` |
|---|---|---|---|
| `initial_fidelity`, `fidelity_step` | live | **inert** | **inert** |
| `perturbation_strength` | live | live | **inert** |
| `restart_*`, `acceptance_tolerance` | live | live | **inert** |
| `random_probes` | live | live | live |

`n_runs` is `n_total` for anything but `Focused`, so the fidelity pair is dead
under `basic` too — this is not only a `random` problem. Under `random` the
perturbation call is replaced outright by `random_config` (`ils.rs:347-352`),
and the `continue` in the `Approach::Random` branch (`ils.rs:420-433`) skips
both the acceptance criterion and the entire restart block that follows it.

Preferred fix: **mark rather than omit**, reusing the `<inactive>` convention
the config diffs already use for guarded-off parameters, so the header stays a
complete record of what was configured while saying what will be read:

```
[    0.02s] fidelity:   <inactive under approach=random>
[    0.02s] perturb:    <inactive under approach=random>
```

A stronger variant is worth considering separately: **warn when the scenario
explicitly sets an option the approach ignores.** Silently accepting a
set-but-ignored option is the same failure class as the wrapper silently
dropping a parameter, which the wrapper-contract item above argues should be an
error rather than a warning — a user who writes `perturbation_strength: 8` under
`approach: random` has a mental model that is wrong, and nothing currently tells
them.

Implementation note: that variant needs more than reading the `Scenario`.
Every one of these fields carries `#[serde(default = …)]` (`scenario.rs:148-209`),
so after deserialization an explicitly-set value is indistinguishable from a
defaulted one. The cheap route is to inspect the raw YAML mapping's keys before
deserializing; the thorough one is `Option<T>` fields resolved after the
approach is known.

---

# Proposed — fail fast when the run cannot possibly work (2026-08-20)

A 24 h tuning run was launched on dai-07 **with no primo binary on PATH and no
instance files in place**. It ran. It reported nothing wrong, and
`ramparils-errors-random.log` stayed **empty**. That is the whole bug: the
budget was spent producing a number, and only a human noticing the missing
files caught it.

## ⬜ Preflight the scenario before spending the budget

Nothing checks that the run *can* work before it starts:

- `load_instances` (`scenario.rs:441`) reads the instance **list** and errors if
  that file is missing — but it never checks that any path *inside* it exists.
  A list of 473 nonexistent `.smt2` files loads cleanly as 473 instances.
- `algo` is never probed. It is handed to `sh -c` at evaluation time
  (`eval.rs:403`), so a missing binary is discovered once per task, forever,
  instead of once at startup.

Add a startup preflight, before the first evaluation, that refuses to start
when: the `algo` command is not runnable; any instance path does not exist or
is unreadable; or a single smoke evaluation on one instance does not return a
parseable `#%# RamParIls #%#` line. Report the first few offending paths rather
than just a count. This is the cheapest item in this file and it would have
turned 24 wasted hours into a one-line error.

## ⬜ Abort when every evaluation returns the cutoff

Why the error log stayed empty, and why the run looked healthy — there are two
paths and both end quietly:

- **Missing result line.** `parse_solver_output` (`eval.rs:488-506`) returns
  `(cutoff_time, 0.0, "UNKNOWN")` when no `#%# RamParIls #%#` line is present,
  which `run_evaluation` does log via `log_crash` (`eval.rs:369-372`). An empty
  error log therefore means this path was *not* taken.
- **So the wrapper answered "normally".** `primo_wrapper.py` evidently turns a
  missing binary into a well-formed no-result line, which ramparils faithfully
  records as a legitimate timeout at the cutoff. Nothing is crashing, so
  nothing is logged.

Either way the search sees **every configuration scoring exactly
`cutoff_time`** — a perfectly flat objective — and an ILS on a flat objective
does not fail, it just wanders. This is the same symptom the inert-parameter
section above is about ("whole neighbourhoods scoring identically reads as a
plateau"), now reached from a missing *binary* rather than a dead parameter,
which is worth noting because it means the symptom does not identify the cause.

Add a guard: if the first N evaluations (N ~ one neighbourhood) all return
`cutoff_time`, or all carry the same status, abort with a diagnostic naming the
resolved `algo` command and one example instance path. A run in which nothing
is ever solved is never what the user meant, and detecting it costs one counter.

Note this also argues the wrapper contract should be tightened at the same
point: a wrapper that cannot find its solver should say so, not report a
timeout. That is the `--version` / `--list-parameters` family above; this item
is the defence for when the wrapper does not cooperate.

## ⬜ Do not silently reuse a broken run's cache

The same incident has a second half, and the preflight above does not cover it.
After the bad run was noticed and the scenario fixed, the **next** run served
its garbage straight back out of `primo-select-10s-random.dbcache`:

```
[    0.06s] eval: submitted tasks=473    hits=473    misses=0
[    0.11s] eval: submitted tasks=15136  hits=15136  misses=0
[    0.21s] ils: bls local optimum score=10.000000
```

Every task a cache hit, round zero complete in 0.15 s, and the score exactly the
10 s cutoff. 15,136 / 473 = **32 configurations — the starting configuration and
its entire neighbourhood — cached as "times out on all 473 instances".**

Three things make this worse than it first looks:

- **A cache hit is invisible in the score.** The run reports a normal-looking
  number. Only `hits=N misses=0` in a debug line nobody has to read gives it
  away, and only if the reader knows that a whole neighbourhood cannot legally
  be free on a cold start.
- **The poisoned region is the worst one.** It is the neighbourhood of the
  starting configuration, i.e. the single-flip moves the first descent will
  make. Whatever the good options are, they were all just recorded as
  cutoff-level failures.
- **The incumbent self-heals and the cache does not.** Any real local optimum
  beats a cutoff score, so the run looks like it recovers -- while every
  revisit of those 32 configurations keeps returning the fabricated value for
  the rest of the budget.

Recovery required deleting the whole cache, which also threw away every honest
result in it. Three proposals, cheapest first:

- **Report cache composition at startup.** Print entry count and the fraction of
  entries whose runtime is at the cutoff, in the header beside `cache: opened`.
  A cache that is 100% timeouts is then visible before the run commits to it.
  This alone would have caught it.
- **Refuse to serve, or at least warn on, a fully-cached cold start.** The first
  evaluation of a run finding *every* task cached is normal on a resumed run and
  suspicious on a fresh one; combined with "and all of them are at the cutoff"
  it is diagnostic.
- **Give entries enough provenance to be invalidated selectively.** The cache
  key is `hash_config(active_config)` and records nothing about *what produced*
  the result -- which binary, which wrapper, whether the wrapper could even find
  its solver. With that recorded, a bad run's entries could be dropped by
  provenance instead of by deleting the file. This is the cache-side twin of the
  wrapper `--version` item above, and it would also close the separate hazard
  that a cache silently survives a solver upgrade.

`ramparils db` already exports `solved` / `status` / `confs` from a cache, so a
`stats` sub-command (entries, distinct configurations, status histogram, share
at cutoff) has an obvious home, and would turn "delete the cache and lose
everything" into a one-line diagnosis.

---

# Proposed — adaptive capping (2026-08-20)

Two defects in the capping path: the bound is tested against the wrong
statistic, and a partial evaluation is compared as if it were a full one.

**✅ Adaptive capping: test the cumulative sum, not the running mean — done, see `DONE.md`.**

**✅ Never let a partial evaluation win a `dominates` comparison — done, see `DONE.md`.**
Turned out to need a broader fix than passing `n_done` to the two
`dominates` call sites (that alone would only have protected `Focused`,
via its `a_runs >= b_runs` guard — `Basic`/`Random` ignore run counts
entirely and would've stayed exposed): `try_promote_incumbent`/
`accept_or_reject_home_base`/`set_home_base` now refuse an incomplete
evaluation outright, for every approach, before ever comparing scores.
Found for real via `futell`'s aggressive checkpoint capping, not
speculatively — see `DONE.md`'s `futell` entry.

## ⬜ Capping sums even under `overall_obj: median`

`compute_score` honours `overall_obj`, but the capping test always accumulates a
sum. A median run therefore prunes on a statistic it does not score. A sound
test exists: once more than half the instances have come back above the bound,
the final median is above it too. Otherwise disable capping for `median`.

---

# Proposed — `futell` real tuning validation (2026-09-13)

`futell` (checkpoint-based early rejection of BLS neighbours, `DONE.md`) is
implemented, default off, and validated at the mechanism level — replay
simulation matches real checkpoints, the D3 gate holds at its boundary, a
rejected neighbour never wins a round. What it has never had is a real
tuning comparison.

## ⬜ Real tuning comparison before flipping any default

Run `futell: true` against a plain `futell: false` baseline, same scenario,
same `instance_shuffle` (fixed across both sides so shuffle isn't a
confound), and compare final scores — not just eval counts. A couple of
short, real tuning runs with `futell` on have happened since it shipped
(one an explicit 1h bug-shakeout smoke test, one launched to exercise the
acceptance-path fix found the same day), but neither was framed or sized as
this comparison. Still open.

---

# Proposed — `run_obj: solved` (2026-09-12)

Raised while designing the future-telling checkpoint feature (`DONE.md`):
a `RunObjective` that scores a configuration by how many instances it solves,
rather than by runtime or quality.

## ⬜ Wire the existing `SOLVED_STATUSES` classification into live scoring

`src/db.rs` already has exactly the classification this needs —
`SOLVED_STATUSES`/`is_solved()`, the union of TPTP's and SMT-LIB's success
tokens, matching solverpy's `TPTP_OK | SMT_OK` — but it's used only by the
offline `ramparils db solved` export, not by `ils.rs` at evaluation time.
Cheapest path to `run_obj: solved`: reuse that same function live, no
protocol change needed, no existing wrapper touched.

**Two, not three, buckets.** A three-way `solved`/`failed`/`timeout` split
was considered (discussed while designing future-telling) and rejected for
this purpose: `failed` and `timeout` would score identically under any
solved-count objective, so a third bucket adds no scoring information —
only reporting granularity, which the raw `status` string (preserved
verbatim already) already provides better than a collapsed generic label
would (`ResourceOut` vs `GaveUp` vs `Timeout` all becoming the same
`timeout` is a net loss of information for anyone reading `ramparils db
status`). Two buckets — solved / not-solved — are enough.

**Contested since 2026-09-16** — see *"Revisit 'two, not three buckets' under
PAR-k"* above. The reasoning here holds only for a solved-count objective;
under a PAR-k runtime objective the buckets do not score identically.

**This also has a free-standing use beyond `run_obj: solved` itself**:
future-telling's own checkpoint metric (`DONE.md`) independently needs
a cheap "is this solved" signal, and settled on deriving it from the
existing PAR1 wrapper contract (`runtime < cutoff_time`) rather than
`is_solved()`'s status-string matching, specifically so it works for *any*
domain without a closed vocabulary list. Worth keeping in mind if these two
efforts converge — they arrived at the "solved" concept from different
directions (offline export vs. live heuristic) and might end up wanting the
same underlying primitive.

## ⬜ Idea: generic wrapper-reported `solved`/`unsolved` status, for domains outside TPTP/SMT

`SOLVED_STATUSES` is a closed, hardcoded list — it only covers the two
domains this project ships example wrappers for. A wrapper for some future
domain outside TPTP/SMT-LIB would need a RamParILS source change (adding its
success tokens to the hardcoded list) before `run_obj: solved` could work
for it at all.

The more general fix, not yet needed but worth recording: let a wrapper
*optionally* report a generic `solved`/`unsolved` classification directly —
a new, small addition to the protocol alongside the existing free-text
`status` field (which stays exactly as it is, solver-specific and
unconstrained) — so a user with a wrapper for a domain this project has
never heard of can use `run_obj: solved` (and future-telling) without
touching RamParILS's source at all. Only worth building when a concrete
domain outside TPTP/SMT-LIB actually needs it — the closed-list approach
above is simpler and covers everything this project ships today.

**Superseded 2026-09-16** by *"Report the outcome explicitly, rather than
encoding it in `runtime`"* above, which wants the same field for a stronger
reason: not just unknown domains, but correctness in the shipped ones.

---

# Proposed — explicit outcome in the wrapper protocol, and PAR-k scoring (2026-09-16)

Today a wrapper reports **only** `status, runtime, quality`, where `status` is
free text chosen by the wrapper and `runtime` carries the PAR1 penalty for
anything that did not solve: a non-success outcome reports `cutoff_time`
regardless of how long it really took (`examples/*/`'s wrappers, and
`eval.rs`'s own crash path). That convention is settled and works for
scoring — but it means **a duration and a penalty are the same number**, and
nothing downstream can tell them apart without a domain-specific status list.

This is not hypothetical. The checkpoint-horizon bug fixed in `cbe0775` was
exactly this confusion: `CheckpointTracker` read a PAR1 penalty as if it were
elapsed time, so one instance that failed in 50ms but reported the full
cutoff convinced it that a whole horizon had passed, and its early-rejection
verdicts collapsed onto a handful of instances. The fix there keeps the
inference local (only a solved result's runtime is treated as a duration),
but the ambiguity itself is still in the protocol, waiting for the next
consumer to trip over it.

## ⬜ Report the outcome explicitly, rather than encoding it in `runtime`

Add a generic, solver-agnostic outcome to the protocol line — `solved` /
`unsolved` / `failed`, and plausibly `timeout` as a distinct fourth value —
alongside the existing free-text `status`, which stays exactly as it is
(solver-specific, unconstrained, and better for reporting than any collapsed
label). Then:

- `runtime` can mean **real elapsed time, always**, and a consumer that wants
  a duration can have one.
- PAR1 becomes something the *consumer* derives from the outcome, instead of
  something every wrapper has to bake in identically and correctly.
- `run_obj: solved` (below) and `futell`'s checkpoint metric stop needing
  either a hardcoded status vocabulary (`SOLVED_STATUSES`) or the
  `runtime < cutoff_time` proxy — both of which are workarounds for the
  missing field, arrived at independently from opposite directions.

This supersedes the deferred *"generic wrapper-reported `solved`/`unsolved`
status"* idea below, which was parked as "only worth building when a concrete
domain outside TPTP/SMT-LIB needs it". The motivation is no longer only about
unknown domains: the field is needed for correctness in the domains already
shipped.

## ⬜ `penalty` in the scenario: PAR2 and beyond

With an explicit outcome, the penalty stops being hardwired at PAR1 and
becomes a scenario setting — `penalty: 2` scoring an unsolved run as
`2 * cutoff_time` (PAR2), and so on for any k. This is currently impossible
to express: since the wrapper has already folded PAR1 into `runtime`, the
tuner cannot recover "did not solve" to re-weight it, and the entire cache is
full of numbers that mean PAR1-at-the-cutoff-it-was-measured-at and nothing
else.

PAR2 is the standard in much of the configuration/competition literature, and
a heavier penalty is often exactly what a timeout-heavy tuning run needs to
stop treating "almost solved it" and "hopeless" as equally bad.

## ⬜ Make the cache's timeout handling depend on the outcome, not a status string

`adapt_cached_result` (`src/cache.rs`) already tries to reuse a cached result
at a *different* cutoff than it was measured at, and that is exactly the
right idea: a timeout at cutoff X tells you nothing about cutoff Y > X (it
must be re-run), while a genuine solve in 3s is valid at any cutoff ≥ 3s, and
a solve in 7s re-requested at cutoff 5 is a synthetic timeout.

But it decides which case it is with `status.eq_ignore_ascii_case("timeout")`
— a literal string **no shipped wrapper actually emits**. The eprover wrapper
passes through `ResourceOut` / `GaveUp` / `UNKNOWN`; the only producer of
literal `TIMEOUT` is that same function's own synthetic path. The SMT side may
match by coincidence of vocabulary, which is worse than not matching at all,
since it makes the behaviour differ per domain.

The observable consequence: a real timeout stored at cutoff 5 and re-requested
at cutoff 10 fails the status test, fails `runtime > requested_cutoff`, and is
served **verbatim as though it were a genuine 5-second solve**. The `put`
upgrade policy (`keep existing unless the existing row is a timeout being
upgraded`) rests on the same string and is equally inert for that wrapper.
Within one scenario the cutoff is fixed so this stays hidden; it bites exactly
when a cache is reused across cutoffs, which is the case the function exists
to serve.

An explicit outcome field makes all of this sound, and makes PAR-k rescoring
of cached results possible at all: store the outcome and the cutoff it was
measured at, and any k can be applied afterwards without re-running anything.

## ⬜ Revisit "two, not three buckets" under PAR-k

The `run_obj: solved` section below rejected a three-way
`solved`/`failed`/`timeout` split on the grounds that *"`failed` and `timeout`
would score identically under any solved-count objective, so a third bucket
adds no scoring information"*. That premise is sound for solved-count
scoring and false under PAR-k: a timeout genuinely consumed the cutoff, a
crash consumed almost nothing, and whether those two should carry the same
penalty is a real modelling choice a scenario might want to make (and one the
checkpoint simulation, which models an unsolved instance as occupying its
worker for the full cutoff, would read differently for each). Worth deciding
deliberately rather than inheriting the earlier decision, which was made for
a different objective.

---

**The wrapper failure-reporting contract (UNKNOWN sentinel + PAR1 for crashes),
established 2026-08-21, has moved to `DONE.md`** — it's a settled convention
now followed by both example wrappers, not an open task. The section above
proposes revising it; until that happens the convention stands as documented.
