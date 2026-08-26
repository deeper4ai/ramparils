# RamParILS — Completed Work

Archive of finished implementation work and established conventions, split out of
`TODO.md` (2026-08-21) so the active list stays short. Nothing here is a task to pick
up — see `TODO.md` for what's still open.

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
- If `partial_sum > bound_multiplier × incumbent_score × n_instances` → call
  `scheduler.reset()`, mark neighbor as pruned, move on
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
- ✅ **Python smoke test** — `examples/eprover/run.py`: calls `ramparils.specialize()` with eprover/bushy010 data; runnable from any directory (note: this smoke script was later replaced by the grackle-free `eprover_wrapper.py` example — see `examples/eprover/README.md`)

---

## Suggested implementation order (historical — all steps done)

1. `params.rs` — everything else depends on having a working param space
2. `cache.rs` — straightforward SQLite plumbing; needed by eval workers
3. `eval.rs` — `Scheduler` with crossbeam channels + worker threads + solver subprocess; test with stub solver
4. `ils.rs` — neighbourhood → perturbation → dominates → `basic_local_search` (drives scheduler) → `run`
5. `main.rs` — wire up the pieces; smoke-test against the Ruby example scenario
6. `python.rs` — thin wrapper once `ils::run` works
7. Iterative deepening — last, as it's an optimization on top of a working ILS

---

## Wrapper `--version` protocol

Give algorithm wrappers a `--version` convention: `python3 primo_wrapper.py
--version` prints the wrapper's own revision and the solver's version block
(for `primo_wrapper.py`, the output of `primo --version`). `ramparils` calls it
once at launch and writes the result into the debug log header, beside the
`binary:` / `git:` lines it already emits.

Implemented: `probe_wrapper_version()` (`lib.rs`) runs the wrapper's
`--version` at startup from `main.rs` and `python.rs`, checks the exit code,
requires a `supports:` line containing `version`, logs the result separated
from the run's other stats, and refuses to start otherwise. When the wrapper
can't reach its solver it prints a `<solver> MISSING` line right after its own
version block, so a missing binary is caught in under a second instead of
producing 24 hours of flat-objective silence (see TODO.md's "fail fast" section,
which this item was the first fix for). `eprover_wrapper.py` and
`primo_wrapper.py` both implement the convention now — use them as the
reference shape for a new wrapper.

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

---

## Adaptive capping: test the cumulative sum, not the running mean

Was: cap when `partial / len > bound_multiplier × incumbent_score`. Results
arrive in completion order — fastest first — so that running mean is a lower
bound on the true mean and converges only at the end. The cap was sound by
accident and useless in practice: it fired at the finish line, after the work
was paid for. At the other extreme there was no minimum sample, so a single
instance above the bound capped a configuration outright.

Now: cap when `partial_sum > bound_multiplier × incumbent_score × n_instances`
— the budget beating the incumbent allows. Costs never go down, so passing it
proves the final mean exceeds the bound. Exact rather than heuristic, and both
failure modes go: no cap before `B/C` of the set, and none at the finish line.

Also done, from the same work:

- **A capped score is no longer printed as if it were a score.**
  `ConfigEvaluation` carries `n_done` and renders as `>0.005220 (1/21)` — the
  `>` because a capped mean is a lower bound, the ratio because a cap after 1
  instance and a cap after 470 are not the same claim.
- **An end-of-run `ils: summary` line** with
  `rounds / searched / gated / incumbents / evals / capped`; a *gated* round is
  one whose start was capped and which then accepted no move.

---

# Wrapper failure-reporting contract — established (2026-08-21)

Building `eprover_wrapper.py` (a grackle-free E prover wrapper for
`ramparils`) surfaced a wrapper-side failure class distinct from the dead-
parameter one above: **a wrapper that reports a genuine crash as if it were a
normal result.** Two concrete bugs, both fixed in `eprover_wrapper.py` and
worth stating as the contract every future wrapper should follow rather than
re-discovering:

- **A crash must charge the full cutoff, never the real elapsed time.**
  `run_obj: runtime` means a wrapper that returns a crash's true (possibly
  near-instant) runtime lets crashes score *better* than genuine solves — the
  ILS then climbs toward configurations that reliably fail fast. PAR1: failed,
  timed-out, and errored runs all report `runtime = cutoff`.
- **Report a genuine crash as `status = "UNKNOWN"`, not an invented status.**
  `ramparils`'s `parse_solver_output` (`eval.rs`) already treats `UNKNOWN` as
  the *only* status that both logs to the error log (`log_crash`) and is
  excluded from the cache — this exists for the missing-`#%# RamParIls #%#`-
  line case, but a wrapper can deliberately emit it for its own crashes too.
  An early version of `eprover_wrapper.py` instead invented its own `"ERROR"`
  status; ramparils didn't recognise it as special, so every crash was cached
  as a legitimate result and the error log — the one place a human would have
  noticed — stayed empty. Reusing `UNKNOWN` needed zero Rust changes.

**How this was found**: two stale domain values inherited from an older
grackle-based parameter file (`sel=SelectNoLiterals`, `tord_prec=invfreqrank`)
turned out to be invalid against the actual E build (confirmed via `eprover -W
none` / `-G none`, which print the accepted-values list as part of a "wrong
argument" error) and made E exit nonzero on ~43% of evaluations. With the two
bugs above both present, that 43% silently scored better than real solves and
would have been cached as such forever. Fixed the domain (dropped both
values) and the wrapper (PAR1 + `UNKNOWN`) together; see
`examples/eprover/eprover_wrapper.py` and `examples/eprover/params-eprover.txt`
for the corrected reference.

---

# Test forbidden clauses against the active projection (2026-08-26)

`random_config` and `neighbourhood` (`src/ils.rs`) used to call
`space.is_forbidden(&cfg)` on the **full**, unprojected config — including
whatever value each currently-inactive parameter happened to be carrying.
Since only `active_config(config, space)` is ever hashed, cached or sent to
the wrapper (`evaluate_config_outcome`, `ils.rs:919`), a forbidden clause that
names a parameter which is inactive in a given draw could reject a
configuration whose *active* projection was perfectly legal — a spurious
rejection, biasing the sampled distribution and (per the still-open item
above) an unmeasured amplifier of the unbounded-rejection-loop hang.

Fixed by testing the active projection instead, at both call sites:

```rust
if !space.is_forbidden(&active_config(&cfg, space)) { ... }       // random_config
if !space.is_forbidden(&active_config(&new_cfg, space)) { ... }   // neighbourhood
```

`active_config` was already computed the same way at the evaluation call site,
so this makes all three call sites agree on what "forbidden" means. Three new
tests in `ils.rs`'s test module cover the shape of the bug and its inverse:

- `forbidden_clause_on_inactive_parameter_is_not_a_real_constraint` — a raw
  config matches a clause naming an inactive parameter, but its active
  projection does not.
- `neighbourhood_does_not_reject_move_due_to_inactive_forbidden_match` — a
  move that deactivates a parameter must not be blocked by a clause naming
  that parameter's now-irrelevant stale value.
- `neighbourhood_catches_forbidden_combo_exposed_by_activating_a_shared_guard`
  — the fix's flip side: two children of the same guard can each hold a
  value that is individually harmless while both are inactive, but forbidden
  in combination; the single move that activates their shared guard must
  still be rejected, since `active_config` is recomputed fresh from the
  post-move config and picks up both newly-active values before the check.
- `random_config_does_not_reject_due_to_inactive_forbidden_match` — 200 draws
  against a space with such a clause, none of which should ever fail the
  active-projection check.

Does not touch `validate_initial`'s forbidden check (`params.rs:207-208`),
which still tests the raw caller-supplied config — that check is already
*too strict* (over-rejecting), the opposite direction from this bug, and
changing it wasn't asked for here.
