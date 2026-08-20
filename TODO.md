# RamParILS — Implementation TODO

Legend: ✅ done · 🏗 scaffolded (types/stubs exist, logic missing) · ⬜ not started

---

## Infrastructure & scaffolding

- ✅ **Cargo.toml** — all dependencies declared (rayon, rusqlite, clap, serde, serde_yaml, pyo3)
- ✅ **pyproject.toml** — maturin build config for Python extension
- ✅ **Module layout** (`lib.rs`) — all modules declared, feature flag for PyO3
- ✅ **Add `crossbeam` dependency** — `crossbeam = "0.8"` in Cargo.toml

---

## `scenario.rs` — Scenario loader

- ✅ `Scenario` struct with all fields (`algo`, `paramfile`, `instance_file`, `cutoff_time`, `tuner_timeout`, `run_obj`, `overall_obj`)
- ✅ `Scenario::from_file()` — YAML deserialization via serde_yaml
- ✅ `RunObjective` / `OverallObjective` enums with sane defaults
- ✅ **`scenario::load_instances(path)`** — reads instance file line-by-line, strips blank lines and `#` comments

---

## `params.rs` — Parameter space parser

- ✅ `Param`, `Condition`, `Forbidden`, `ParamSpace` structs
- ✅ **`Config` type alias** — moved here (lowest-level module); `ils.rs` re-exports it
- ✅ **`ParamSpace::from_file()`** — single-pass parser: param lines (`name {domain} [default] | condition`), forbidden lines (`{p=v, …}`), inline `#` comments stripped
- ✅ **`ParamSpace::default_config()`** — all params at their default values
- ✅ **`ParamSpace::active_params(config)`** — iterates to fixpoint for transitive conditionals
- ✅ **`ParamSpace::is_forbidden(config)`** — checks all forbidden combos
- ✅ **Unit tests** — SAPS params, conditional, forbidden, active_params, default_config

---

## `cache.rs` — Persistent result cache

### Schema

Normalized to avoid storing path strings in every result row — critical at 100k instances:

```sql
CREATE TABLE IF NOT EXISTS instances (
    id   INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL
);
CREATE TABLE IF NOT EXISTS strategies (
    hash   INTEGER PRIMARY KEY,
    config TEXT NOT NULL          -- JSON object, sorted keys; makes the cache self-describing
);
CREATE TABLE IF NOT EXISTS results (
    strategy_hash INTEGER NOT NULL,
    instance_id   INTEGER NOT NULL,
    runtime       REAL    NOT NULL,
    PRIMARY KEY (strategy_hash, instance_id)
);
```

`results` rows are three integers — compact B-tree index, fast integer comparisons.

### Tasks

- ✅ `Cache` struct with `rusqlite::Connection`
- ✅ **`Cache::open(path)`** — opens/creates DB; runs DDL; WAL + NORMAL sync for performance
- ✅ **`Cache::load_instances(instances: &[String]) -> HashMap<String, i64>`** — `INSERT OR IGNORE` + bulk `SELECT id`; idempotent
- ✅ **`Cache::get_batch(hash, instance_ids) -> HashMap<i64, CachedResult>`** — single bulk query per strategy; returns `(runtime, quality)` per hit
- ✅ **`Cache::put(hash, instance_id, runtime, quality)`** — `INSERT OR REPLACE`
- ✅ **`cache::hash_config(config: &Config) -> u64`** — sorts keys, uses `DefaultHasher`; stable within a run
- ✅ **Unit tests** — open, load_instances round-trip, get_batch miss→hit, put replace, hash stability

---

## `eval.rs` — Parallel evaluation scheduler

Parallelism is over `(neighbor, instance)` pairs, not just instances. With N neighbors × I
instances = N×I tasks, all 60 workers fill immediately — including the single-instance case
(60 neighbors × 1 instance = 60 parallel solver calls).

### Types

- ✅ `EvalResult` struct with `mean()` and `median()`
- ✅ **`EvalTask`** — `{ neighbor_id, config, hash, instances: Vec<(i64, String)> }`
- ✅ **`WorkItem`** — internal; `{ neighbor_id, config, hash, instance_id, instance_path, cutoff_time }`
- ✅ **`TaskResult`** — `{ neighbor_id, instance_id, hash, runtime, quality }` — carries both for cache write-back by ILS

### `Scheduler`

- ✅ **`Scheduler::new(n_workers, algo, cutoff_time)`** — spawns worker threads; crossbeam unbounded channels; stop flag via `Arc<AtomicBool>`
- ✅ **`Scheduler::submit(&self, tasks, cache)`** — bulk cache read per task; hits → result channel instantly; misses → work channel for workers
- ✅ **`Scheduler::results()`** — `&Receiver<TaskResult>` for ILS
- ✅ **`Scheduler::reset()`** — sets stop flag; workers drain work channel without processing
- ✅ **Cache write-back** — ILS calls `cache.put()` per `TaskResult`; keeps cache access single-threaded

### Solver integration

- ✅ **`run_solver_inner(algo, config, instance, cutoff_time) -> (runtime, quality)`** — `sh -c` invocation; parses `Result for ParamILS:` / `Result for SMAC:` line; caps at `cutoff_time`; returns `(cutoff_time, 0.0)` on crash

### Adaptive capping (ILS-side, not scheduler-side)

Capping is checked by the ILS loop as results arrive, not inside the scheduler:
- Accumulate partial sum per neighbor
- If `partial_mean > bound_multiplier × incumbent_mean` → call `scheduler.reset()`,
  mark neighbor as pruned, move on
- This keeps the scheduler stateless and reusable

---

## `ils.rs` — ILS algorithm

- ✅ `Config` type alias (re-exported from `params`), `Approach` enum, `IlsOptions` struct
- ✅ **`neighbourhood(config, space)`** — active params only; skips forbidden combos
- ✅ **`perturbation(config, strength, space)`** — `strength` random neighbourhood steps (random walk, same as Ruby); `Approach::Random` → fresh random config
- ✅ **`dominates(a_score, a_runs, b_score, b_runs)`** — BasicILS: score only; FocusedILS: runs ≥ and score ≤
- ✅ **`basic_local_search()`** — submits all neighbours in one batch; accepts first fully-evaluated dominating neighbour; adaptive capping per neighbour; `scheduler.reset()` on acceptance
- ✅ **`weakly_dominates()`** — ParamILS's `equalIsBetter=true` variant; used only by the acceptance criterion
- ✅ **`acceptance_criterion()`** — accept new LM if it weakly dominates the last LM (ties go to the challenger, "moving away from incumbent")
- ✅ **Fidelity consistency** — incumbent *and* ILS home base are both re-measured on every fidelity increase, so no comparison spans two different instance prefixes; regression test in `tests/focused_fidelity.rs`
- ✅ **`run()`** — creates `Scheduler` internally; initializes from provided config or best of 10 random; BLS → perturbation → acceptance loop until timeout; returns global incumbent
- ✅ **`compute_score()`** — mean or median of runtime or quality based on `IlsOptions`
- ✅ **`random_config()`** — rejection-sample non-forbidden configs
- ✅ **Unit tests** — neighbourhood size/forbidden, perturbation, dominates (Basic + Focused), compute_score (mean + median), random_config
- ✅ **Detail levels** (FocusedILS) — adaptive run count (`n_runs`) starts at 1, grows by 1 each time the incumbent survives a challenge; BLS evaluates neighbours on `instances[..n_runs]`; mirrors `boundedIncTimeSpentInState()` logic
- ✅ **Iterative deepening** — `iterative_deepening_ils()`: exponential schedule over N, cutoff_time, tuner_timeout (λ_n, λ_c, λ_t)

---

## `main.rs` — CLI entry point

- ✅ `Args` struct with all flags mirroring Ruby CLI (`--scenariofile`, `--numRun`, `--approach`, `--ps`, `--bm`, `--pruning`, `--id`, `--cachedb`, `--cores`, `--debug`)
- ✅ **`main()`** — loads scenario → param space → instances → cache → runs ILS from default config → prints result as `-key val …` (active params only, sorted)

---

## `python.rs` — PyO3 bindings

- ✅ Module registration (`#[pymodule]`)
- ✅ `specialize()` signature with correct `#[pyo3(signature = …)]`
- ✅ **`specialize()` body** — loads param space, instances, cache; runs ILS (FocusedILS defaults); returns active params as `dict[str, str]`
- ✅ **Error mapping** — `anyhow::Error` → `PyRuntimeError` via `run_specialize()` helper
- ✅ **Fix PyO3 unsafe warnings** — upgraded to pyo3 0.23 which fixed `unwrap_required_argument` being flagged as unsafe in Rust 2024 edition

---

## Testing

- ✅ **Unit tests for `params.rs`** — inline tests: SAPS params, conditional, forbidden, active_params, default_config
- ✅ **Unit tests for `cache.rs`** — inline tests: open in-memory DB, round-trip get/put, hash stability
- ✅ **Unit tests for `eval.rs`** — inline tests: output parsing, scheduler cache-hit path, scheduler reset
- ✅ **Integration test** — `tests/eprover.rs`: parse real `params-eprover.txt` (34 params, 5 standalone conditionals), run full FocusedILS on eprover/bushy010 for 15 s; assert config valid
- ✅ **Python smoke test** — `examples/eprover/run.py`: calls `ramparils.specialize()` with eprover/bushy010 data; runnable from any directory

---

## Suggested implementation order

1. `params.rs` — everything else depends on having a working param space
2. `cache.rs` — straightforward SQLite plumbing; needed by eval workers
3. `eval.rs` — `Scheduler` with crossbeam channels + worker threads + solver subprocess; test with stub solver
4. `ils.rs` — neighbourhood → perturbation → dominates → `basic_local_search` (drives scheduler) → `run`
5. `main.rs` — wire up the pieces; smoke-test against the Ruby example scenario
6. `python.rs` — thin wrapper once `ils::run` works
7. Iterative deepening — last, as it's an optimization on top of a working ILS

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

## ⬜ Wrapper `--version` protocol

Give algorithm wrappers a `--version` convention: `python3 primo_wrapper.py
--version` prints the wrapper's own revision and the solver's version block
(for `primo_wrapper.py`, the output of `primo --version`). `ramparils` calls it
once at launch and writes the result into the debug log header, beside the
`binary:` / `git:` lines it already emits.

Two independent reasons, both now evidenced:

- **The solver build is what an experiment's premise rests on, and nothing in a
  run's own record pins it.** Runs 3–4 ran primo `0c0843d` and runs 5–6
  `996a100`; the same configuration scored 4–8% better under the newer build,
  so no score crosses a solver upgrade safely. Today only an out-of-band
  `versions.txt` sync records this.
- **It would have caught the stale wrapper.** `ramparils --version` cannot
  attest the wrapper at all — the wrapper is a Python file invoked at runtime,
  not compiled in — so `--version` and `hostinfo.sh` are blind exactly where
  the parameter space depends on it.

Already solved, do not redo: `ramparils` stamps its *own* revision (git sha,
`-dirty`, profile, rustc) into `--version` and the log header via `build.rs`,
from `fde14be`.

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

## ⬜ Test forbidden clauses against the active projection

`random_config` calls `space.is_forbidden(&cfg)` on the **full** draw
(`ils.rs:1268`), so a
clause mentioning a parameter that is *inactive* in that draw can reject a
configuration whose active projection is perfectly legal. Since only the active
projection is ever evaluated, hashed or sent to the solver, that rejection is
spurious.

Two effects, and it is worth separating them because the first is certain and
the second is a hypothesis:

- **Certain**: it biases the sampled distribution, on top of the
  non-uniformity in the next item.
- **Hypothesis**: it is an *amplifier* of the hang above. Under
  `is_forbidden(active_config(&cfg, space))` a clause naming an inactive
  parameter cannot match at all — `config.get(param)` returns `None`, so
  `params.rs:259`'s `config.get(param) != Some(val)` skips the clause — and the
  projected test
  rejects a strict subset of what the current one rejects, and the legal
  fraction p can only go up. How much depends on whether a given space's
  clauses mention conditional parameters, so this narrows the failure without
  removing it. Do the previous item regardless.

`neighbourhood` (`ils.rs:628`) tests the full config too, and should be changed
with it or deliberately left alone; decide once and document which.

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
