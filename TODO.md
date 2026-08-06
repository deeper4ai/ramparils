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
