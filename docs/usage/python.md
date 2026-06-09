# Python API

```python
import ramparils
```

The Python extension provides a single function, `specialize`, that runs FocusedILS from an
initial strategy and returns the best configuration found within the time budget.  It is a
native Rust extension built with PyO3, so there is no subprocess overhead — the ILS loop,
parallel evaluation, and SQLite cache all run in-process.  All tuning options are passed as
fields in the `scenario` dict, matching the YAML keys documented in the [CLI reference](cli.md).

## specialize

```python
ramparils.specialize(strategy, scenario) -> dict[str, str]
```

Specialize a strategy on a set of benchmark instances using ILS.

Runs Iterated Local Search starting from `strategy`, evaluating `(configuration, instance)` pairs
in parallel, and returns the best configuration found within the time budget.  Results are cached
in a persistent SQLite database so that repeated calls with the same algorithm and instances never
redo solver invocations.

### Arguments

**`strategy`** — `dict[str, str]`

Initial parameter configuration as `{name: value}` strings.  Must contain every parameter
defined in `scenario["paramfile"]`.  Values are strings even for numeric parameters
(e.g. `"1.189"`).

**`scenario`** — `dict`

Tuning scenario.  All keys match the YAML fields in the [CLI reference](cli.md).

**Required keys:**

| Key | Type | Description |
|-----|------|-------------|
| `algo` | str | Command to invoke the target algorithm. Invoked as `<algo> <instance> <cutoff_time> -p1 v1 …` |
| `paramfile` | str | Path to the `.params` file describing the parameter space. |
| `cutoff_time` | float | Per-run time limit in seconds, passed to the target algorithm. |
| `tuner_timeout` | float | Total wall-clock budget for the tuner in seconds. |

Exactly one of the following must be supplied to specify instances:

| Key | Type | Description |
|-----|------|-------------|
| `instances` | list[str] | List of instance paths directly. |
| `instance_file` | str | Path to a text file with one instance path per line. |

**Optional keys** (all have defaults matching the CLI):

| Key | Default | Description |
|-----|---------|-------------|
| `run_obj` | `"runtime"` | `"runtime"` or `"quality"`. |
| `overall_obj` | `"mean"` | `"mean"` or `"median"`. |
| `approach` | `"focused"` | `"focused"`, `"basic"`, or `"random"`. |
| `perturbation_strength` | `4` | Neighbourhood steps per perturbation. |
| `initial_fidelity` | `1` | Initial instances per configuration in FocusedILS, capped by the instance count. |
| `fidelity_step` | `1` | Instances added at each FocusedILS fidelity increase. |
| `bound_multiplier` | `10.0` | Adaptive capping multiplier. |
| `pruning` | `True` | Enable adaptive capping. |
| `iterative_deepening` | `False` | Enable iterative deepening. |
| `lambda_n` | `0.5` | Iterative deepening instance-count factor. |
| `lambda_c` | `0.5` | Iterative deepening cutoff-time factor. |
| `lambda_t` | `0.5` | Iterative deepening timeout factor. |
| `cores` | `0` | Parallel workers; `0` uses all available cores. |
| `num_run` | `0` | Run index / random seed (reserved). |
| `cache_db` | `":memory:"` | Path to the SQLite cache. Defaults to in-memory (not persisted). Set to a file path to share results across calls. |
| `debug` | `False` | Print new incumbents and scores to stderr. |
| `debug_wrapper` | `False` | Print every solver invocation. |
| `debug_solver` | `False` | Print every solver result. |
| `debug_log` | `None` | Write debug output to this file. |
| `error_log` | `None` | Write failed solver runs to this file. |
| `test_instance_file` | `None` | Reserved for future use. |

### Returns

`dict[str, str]` — The best configuration found.  Only *active* parameters are included
(inactive conditional parameters are omitted).

### Raises

`RuntimeError` — If the scenario is invalid, the instance list is empty, the paramfile cannot
be parsed, or the cache cannot be opened.

## Example

```python
import ramparils

result = ramparils.specialize(
    strategy={
        "alpha": "1.189",
        "rho":   "0.5",
        "ps":    "0.1",
        "wp":    "0.03",
    },
    scenario={
        # Required
        "algo":          "ruby /path/to/saps_wrapper.rb",
        "paramfile":     "/path/to/saps.params",
        "instances":     [
            "/path/to/instances/inst1.cnf",
            "/path/to/instances/inst2.cnf",
        ],
        "cutoff_time":   5.0,
        "tuner_timeout": 120.0,
        # Optional — defaults shown
        "cache_db":      "/tmp/ramparils_cache.db",
        "cores":         0,       # 0 = all available
        "approach":      "focused",
        "debug":         False,
    },
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
```

See `examples/saps_python.py` for a runnable example.
