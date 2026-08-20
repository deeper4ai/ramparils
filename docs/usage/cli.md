# ⌨️ CLI

One binary with two sub-commands: `run` tunes, `db` exports a cache.

```bash
ramparils run path/to/scenario.yaml
```

All tuning options — instances, cutoff times, algorithm settings, cache location, debug flags —
live in a single YAML scenario file.  `run` takes the scenario path and nothing else: every knob
is a field in the YAML, making scenarios self-contained, reproducible, and easy to share.

```bash
ramparils db results.dbcache             # all three at once

ramparils db solved  results.dbcache     # or one at a time
ramparils db status  results.dbcache
ramparils db confs   results.dbcache [--json]
```

`db` writes one file per strategy hash, named `ram-<hash>`, under `--out-dir` (default
`solverpy_db`), in a layout that mirrors solverpy's database:

| sub-command | writes | content |
|---|---|---|
| `solved` | `<out-dir>/solved/<dbcache-stem>/ram-<hash>` | one instance path per line |
| `status` | `<out-dir>/status/<dbcache-stem>/ram-<hash>` | `instance <TAB> status <TAB> runtime` |
| `confs` | `<out-dir>/confs/<dbcache-stem>/ram-<hash>` | the configuration, as YAML |

Each prints a one-line summary on stdout; stderr carries errors only. Given a cache and no
sub-command, `db` runs all three, so `ramparils db results.dbcache` is the usual invocation and the
sub-commands are there for when you want one of them or `--json`.

A cache whose filename happens to be `solved`, `status` or `confs` is read as the sub-command
instead; write `./status` to disambiguate.

`solved` counts a result as solved when its status is one of `Theorem`, `Unsatisfiable`,
`Satisfiable`, `CounterSatisfiable`, `ContradictoryAxioms`, `sat` or `unsat` — the union of TPTP's
and SMT-LIB's success tokens, as solverpy uses. RamParILS itself stores the status verbatim and
never interprets it when scoring, so a target algorithm reporting anything else is not recognised
here and its instances are reported as unsolved.

`confs/` is deliberately not solverpy's `strats/`: a strategy file there holds a solver command
line, a conf file here holds a parameter assignment, which only means anything against the
parameter space it was tuned in. It records the **active** configuration — parameters whose guard
was closed are absent, because that is what the cache keys on, and what collapses a guarded
sub-space to a single entry. A conf file is therefore a record of what ran rather than a complete
configuration, and [`initial_config_file`](#initial-configuration) will reject it unless every
parameter in the space happened to be active.

```console
$ ramparils --version
ramparils 0.2.0
git:   6d967a5
build: release, rustc 1.97.1 (8bab26f4f 2026-07-14), x86_64-unknown-linux-gnu
```

The revision is baked in at build time, and it matters more than the version: `0.2.0` covers every
commit since that tag, so the version alone does not say which code ran. A `-dirty` suffix means
the worktree had uncommitted changes and the revision therefore does not identify the build. Where
the crate is built without git — an sdist, a source tarball — the revision reads `unknown` rather
than failing the build. The same lines open every debug log:

```text
[    0.00s] binary:  ramparils v0.2.0
[    0.00s] git:     6d967a5
[    0.00s] build:   release, rustc 1.97.1 (8bab26f4f 2026-07-14), x86_64-unknown-linux-gnu
```

---

## 🧭 Scenario file reference

```yaml
# Required: use instance_file or instances
algo:          "ruby /path/to/solver_wrapper.rb"
paramfile:     "/path/to/solver.params"
instance_file: "data/train.txt"
# instances:   ["data/one.cnf", "data/two.cnf"]
initial_config:
  engine: quick
  threads: 4
# initial_config_file: "initial-config.yaml"  # alternative to initial_config
cutoff_time:   5.0
tuner_timeout: 300.0

# Optional — shown with defaults
run_obj:               runtime   # runtime | quality
overall_obj:           mean      # mean | median
approach:              focused   # focused | basic | random
perturbation_strength: 4
restart_probability:   0.0       # ParamILS p_restart; 0 = never
restart_failures: 0        # restart after k rejected local optima; 0 = never
restart_target:        incumbent # incumbent | random
restart_strength:      0         # 0 = 2 * perturbation_strength
acceptance_tolerance:  0.0       # accept within this margin of the incumbent
random_probes:         0         # ParamILS R; 0 = start from the given config only
initial_fidelity:      1
fidelity_step:         1
bound_multiplier:      10.0
pruning:               true
iterative_deepening:   false
lambda_n:              0.5
lambda_c:              0.5
lambda_t:              0.5
cores:                 0         # 0 = all available
num_run:               0
cache_db:              ":memory:"    # use a file path to persist across runs
debug:                 false
debug_wrapper:         false
debug_solver:          false
debug_log:             ~         # path or null
error_log:             ~         # path or null
```

### 🔑 Required fields

| Field | Type | Description |
|-------|------|-------------|
| `algo` | string | Shell command used to invoke the target algorithm. Invoked as `<algo> <instance> <cutoff_time> -p1 v1 …` via `sh -c`. |
| `paramfile` | string | Path to the `.params` file describing the parameter space (domains, defaults, conditionals, forbidden combinations). |
| `instance_file` | string | Path to a text file listing training instance paths, one per line. Blank lines and `#` comments are ignored. |
| `instances` | list of strings | Inline training-instance paths. This works in YAML as well as Python. If both instance fields are set, `instances` takes precedence. |
| `cutoff_time` | float | Per-run time limit in seconds. Passed to the target algorithm; the solver wrapper is expected to respect it. |
| `tuner_timeout` | float | Total wall-clock budget for the tuner in seconds. RamParILS stops launching new evaluations once this is exceeded and returns the best configuration found. |

Paths in the scenario, parameter file, and instance list are interpreted from
the directory where `ramparils` is started, not from the scenario file's
directory. Use absolute paths or run from a documented working directory when
the scenario must be portable.

### Initial configuration

Use either `initial_config` for an inline YAML mapping or
`initial_config_file` for a file containing the same mapping:

```yaml
initial_config:
  engine: quick
  threads: 4
  use_preprocessing: true
```

```yaml
initial_config_file: "initial-config.yaml"
```

The two fields are mutually exclusive. An explicit initial configuration must
contain every parameter from the parameter file, including conditional
parameters that are initially inactive. Parameter names and values are
validated against the parameter space, and forbidden configurations are
rejected. YAML string, numeric, and boolean scalar values are accepted.

When neither field is present, RamParILS retains its previous behavior and
starts from the defaults in square brackets in the parameter file.
`initial_config_file` is interpreted from the directory where `ramparils` is
started.

### 🎯 Objective

| Field | Default | Description |
|-------|---------|-------------|
| `run_obj` | `runtime` | Which numeric value to minimise: `runtime` or `quality`. For maximisation problems, make the wrapper convert utility to a cost, for example by negating it. |
| `overall_obj` | `mean` | How per-run results are aggregated across instances: `mean` or `median`. `median` is more robust to outliers but ignores magnitude. |

### 🔍 Algorithm

| Field | Default | Description |
|-------|---------|-------------|
| `approach` | `focused` | Search mode. `focused` (default) starts at `initial_fidelity` instances and increases fidelity when the incumbent survives a challenge. `basic` uses all instances from the start. `random` is ParamILS's `pert_rand`: each round starts from a fresh random configuration and the acceptance criterion is skipped, which makes it a random-restart baseline rather than an iterated local search. See [Algorithm](../reference/algorithm.md). |
| `perturbation_strength` | `4` | Number of random parameter changes applied during perturbation to escape a local optimum. Larger values jump further in the space; smaller values stay closer to the current local optimum. |
| `restart_probability` | `0.0` | ParamILS's `p_restart`: probability of restarting the home base after each round. `0` disables it. See [Algorithm](../reference/algorithm.md). |
| `restart_failures` | `0` | Restart the home base after this many consecutive rejected local optima. `0` disables it. Adapts to however many rounds the budget allows, unlike a fixed probability. |
| `restart_target` | `incumbent` | Where a restart lands: `incumbent` perturbs the best configuration found so far by `restart_strength` steps; `random` draws a uniformly random configuration, as ParamILS does. |
| `restart_strength` | `0` | Perturbation steps a restart applies to the incumbent. `0` resolves to `2 × perturbation_strength`; the resolved value is printed in the debug header. |
| `acceptance_tolerance` | `0.0` | Accept a local optimum worse than the home base while it stays within this relative margin of the *incumbent*. `0` keeps the ParamILS rule of accepting only an at-least-as-good local optimum. |
| `random_probes` | `0` | ParamILS's `R`: probe this many random configurations before the first descent, stepping to any that beats the starting configuration. The default of `0` starts from the supplied configuration and nothing else, which is what specializing a caller-supplied strategy requires. |
| `initial_fidelity` | `1` | Initial number of instances used to score each configuration in FocusedILS. Larger values shift worker capacity from speculative neighbor evaluation toward parallel instance evaluation. Values are clamped to the available instance count. |
| `fidelity_step` | `1` | Number of instances added when FocusedILS increases fidelity after the incumbent survives a challenge. Values of `0` are treated as `1`. |
| `bound_multiplier` | `10.0` | Capping threshold. A candidate is abandoned once its running sum exceeds `bound_multiplier × incumbent_score × n_instances` — the budget beating the incumbent allows. Lower values prune more aggressively; capping is exact, so it never discards a configuration that would have been accepted. |
| `pruning` | `true` | Enable capping. Disable it for `overall_obj: median`, where the test sums a statistic the run does not score. |

FocusedILS uses the first N entries from `instance_file`, not a random sample.
Order the file deliberately or shuffle it before a run when early prefixes
should represent the full training set.

### 📈 Iterative deepening

Runs multiple ILS phases with an exponential schedule.  Early phases use fewer instances and a
shorter cutoff to rapidly explore the space; later phases refine the best region with the full
budget.  Useful when the training set is large or `cutoff_time` is long.
See [Iterative deepening](../reference/algorithm.md#iterative-deepening).

| Field | Default | Description |
|-------|---------|-------------|
| `iterative_deepening` | `false` | Enable iterative deepening. |
| `lambda_n` | `0.5` | Geometric instance-count factor. Each later phase grows toward the full instance set; `0.5` produces approximate doubling. |
| `lambda_c` | `0.5` | Geometric cutoff factor. Each later phase grows toward `cutoff_time`; `0.5` produces approximate doubling. |
| `lambda_t` | `0.5` | Geometric cumulative-deadline factor. Each phase receives the time remaining before its scheduled deadline; `0.5` doubles successive deadlines toward `tuner_timeout`. |

### ⚙️ Execution

| Field | Default | Description |
|-------|---------|-------------|
| `cores` | `0` | Number of parallel worker threads. `0` uses all available CPU cores. Set to a specific number to limit parallelism on shared machines. |
| `cache_db` | `":memory:"` | Path to the SQLite cache file. Defaults to an in-memory cache (not persisted). Set to a file path to share cached results across runs on the same benchmark. Cache rows include the execution cutoff, allowing safe reuse across iterative-deepening phases. |
| `num_run` | `0` | Run index, reserved for future use as a random seed. Has no effect currently. |

Cache entries are keyed by the active configuration and instance path, with the
execution cutoff stored on each result. A timeout can satisfy only requests
with an equal or shorter cutoff. A completed result that exceeds a shorter
requested cutoff is returned as an in-memory synthetic timeout and is never
written back. The algorithm command, objective, solver version, wrapper
behavior, and random seed are not included. Use a separate cache if any of them change;
otherwise stale results may be reused without warning.

Caches created before cutoff-aware results were introduced are incompatible
and must be removed or replaced with a new cache file.

### 🩺 Debug

| Field | Default | Description |
|-------|---------|-------------|
| `debug` | `false` | Print structured debug output to stderr: new incumbents, scores, accepted argument changes, and timing. |
| `debug_wrapper` | `false` | Print one line per solver wrapper invocation (instance, parameters). Verbose; useful for tracing evaluation order. |
| `debug_solver` | `false` | Print one line per solver result (status, runtime, quality). Verbose; useful for diagnosing wrapper output. |
| `debug_log` | `null` | Write debug output to this file in addition to (or instead of) stderr. Independent of `debug` — file logging can be active without stderr logging. |
| `error_log` | `null` | Write details of failed solver runs (non-zero exit, missing result line) to this file for post-hoc diagnosis. |
| `test_instance_file` | `null` | Reserved for future use. |

---

## 📤 Output

The complete best configuration is printed to stdout as an alphabetically
ordered YAML mapping:

```yaml
alpha: '1.256'
ps: '0.1'
rho: '0.5'
wp: '0.03'
```

Values are YAML strings so they preserve the parameter-file representation.
Inactive conditional parameters are included, making the output directly
usable as a future `initial_config` or `initial_config_file`.

When `debug_log` is configured, the final YAML mapping is also written there.
Every improved incumbent is recorded in the log with `hash=<hash>` followed
by its complete YAML configuration. The hash identifies the active
configuration used by the evaluation cache.
