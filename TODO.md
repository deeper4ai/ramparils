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
