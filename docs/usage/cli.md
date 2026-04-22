# CLI

```bash
ramparils --scenariofile path/to/scenario.yaml
```

All tuning options — instances, cutoff times, algorithm settings, cache location, debug flags —
live in a single YAML scenario file.  The CLI has no other flags: every knob is a field in the
YAML, making scenarios self-contained, reproducible, and easy to share.

---

## Scenario file reference

```yaml
# Required
algo:          "ruby /path/to/solver_wrapper.rb"
paramfile:     "/path/to/solver.params"
instance_file: "data/train.txt"
cutoff_time:   5.0
tuner_timeout: 300.0

# Optional — shown with defaults
run_obj:               runtime   # runtime | quality
overall_obj:           mean      # mean | median
approach:              focused   # focused | basic | random
perturbation_strength: 4
bound_multiplier:      10.0
pruning:               true
iterative_deepening:   false
lambda_n:              0.5
lambda_c:              0.5
lambda_t:              0.5
cores:                 0         # 0 = all available
num_run:               0
cache_db:              paramils_cache.db
debug:                 false
debug_wrapper:         false
debug_solver:          false
debug_log:             ~         # path or null
error_log:             ~         # path or null
```

### Required fields

| Field | Type | Description |
|-------|------|-------------|
| `algo` | string | Shell command used to invoke the target algorithm. Invoked as `<algo> <instance> <cutoff_time> -p1 v1 …` via `sh -c`. |
| `paramfile` | string | Path to the `.params` file describing the parameter space (domains, defaults, conditionals, forbidden combinations). |
| `instance_file` | string | Path to a text file listing training instance paths, one per line. Blank lines and `#` comments are ignored. Mutually exclusive with `instances` (Python only). |
| `cutoff_time` | float | Per-run time limit in seconds. Passed to the target algorithm; the solver wrapper is expected to respect it. |
| `tuner_timeout` | float | Total wall-clock budget for the tuner in seconds. RamParILS stops launching new evaluations once this is exceeded and returns the best configuration found. |

### Objective

| Field | Default | Description |
|-------|---------|-------------|
| `run_obj` | `runtime` | What a single run optimises: `runtime` (minimise) or `quality` (maximise). Use `quality` when the algorithm always runs to completion and returns a solution value. |
| `overall_obj` | `mean` | How per-run results are aggregated across instances: `mean` or `median`. `median` is more robust to outliers but ignores magnitude. |

### Algorithm

| Field | Default | Description |
|-------|---------|-------------|
| `approach` | `focused` | ILS variant. `focused` (default) uses adaptive dominance-based comparison and is typically 5–20× more efficient than `basic`. `basic` evaluates every configuration on exactly N instances. `random` samples uniformly — useful as a baseline. See [Algorithm](../reference/algorithm.md). |
| `perturbation_strength` | `4` | Number of random parameter changes applied during perturbation to escape a local optimum. Larger values jump further in the space; smaller values stay closer to the current local optimum. |
| `bound_multiplier` | `10.0` | Adaptive capping threshold. A configuration is abandoned early if its accumulated runtime exceeds `bound_multiplier × incumbent_runtime`. Lower values prune more aggressively (faster but riskier); higher values are more conservative. |
| `pruning` | `true` | Enable adaptive capping. Disable only for debugging or when `run_obj: quality` and pruning is not meaningful. |

### Iterative deepening

Runs multiple ILS phases with an exponential schedule.  Early phases use fewer instances and a
shorter cutoff to rapidly explore the space; later phases refine the best region with the full
budget.  Useful when the training set is large or `cutoff_time` is long.
See [Iterative deepening](../reference/algorithm.md#iterative-deepening).

| Field | Default | Description |
|-------|---------|-------------|
| `iterative_deepening` | `false` | Enable iterative deepening. |
| `lambda_n` | `0.5` | Fraction of training instances used in the first phase. Doubles each phase toward 1.0. |
| `lambda_c` | `0.5` | Fraction of `cutoff_time` used in the first phase. Doubles each phase toward the full cutoff. |
| `lambda_t` | `0.5` | Fraction of `tuner_timeout` allocated to the first phase. Doubles each phase. |

### Execution

| Field | Default | Description |
|-------|---------|-------------|
| `cores` | `0` | Number of parallel worker threads. `0` uses all available CPU cores. Set to a specific number to limit parallelism on shared machines. |
| `cache_db` | `paramils_cache.db` | Path to the SQLite cache file. Created if it does not exist. Use the same path across runs on the same benchmark to share cached results. Use `:memory:` for an in-process cache that is not persisted. |
| `num_run` | `0` | Run index, reserved for future use as a random seed. Has no effect currently. |

### Debug

| Field | Default | Description |
|-------|---------|-------------|
| `debug` | `false` | Print structured debug output to stderr: new incumbents, scores, and timing. |
| `debug_wrapper` | `false` | Print one line per solver wrapper invocation (instance, parameters). Verbose; useful for tracing evaluation order. |
| `debug_solver` | `false` | Print one line per solver result (status, runtime, quality). Verbose; useful for diagnosing wrapper output. |
| `debug_log` | `null` | Write debug output to this file in addition to (or instead of) stderr. Independent of `debug` — file logging can be active without stderr logging. |
| `error_log` | `null` | Write details of failed solver runs (non-zero exit, missing result line) to this file for post-hoc diagnosis. |
| `test_instance_file` | `null` | Reserved for future use. |

---

## Output

The best configuration found is printed to stdout as `-param1 val1 -param2 val2 …`
(active parameters only, alphabetically sorted):

```
-alpha 1.256 -ps 0.1 -rho 0.5 -wp 0.03
```

This format is directly compatible with Grackle's strategy parsing.
Inactive conditional parameters are omitted — they are irrelevant given the active parameter values.
