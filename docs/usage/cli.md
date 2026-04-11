# CLI

```bash
ramparils --scenariofile path/to/scenario.yaml
```

## Scenario file

All tuning parameters are set in a YAML file:

```yaml
algo: "ruby /path/to/solver_wrapper.rb"   # command to invoke the target algorithm
paramfile: "/path/to/params.txt"          # parameter space definition
instance_file: "data/train.txt"           # one instance path per line
cutoff_time: 5.0                          # per-run time limit (seconds)
tuner_timeout: 300.0                      # total wall-clock budget (seconds)
run_obj: runtime                          # runtime | quality
overall_obj: mean                         # mean | median
```

`test_instance_file` is an optional second file of instance paths used for final evaluation
(reserved for future use).

## Flags

### Core

| Flag | Default | Description |
|------|---------|-------------|
| `--scenariofile PATH` | *(required)* | Scenario YAML file |
| `--cores N` | all cores | Parallel worker threads (`0` = auto) |
| `--approach` | `focused` | `basic` \| `focused` \| `random` |
| `--bm F` | `10.0` | Adaptive capping bound multiplier |
| `--ps N` | `4` | Perturbation strength |
| `--pruning` | `true` | Enable adaptive capping |
| `--cachedb PATH` | `ramparils_cache.db` | SQLite cache file |
| `--numRun N` | `0` | Run index (reserved, future random seed) |

### Iterative deepening

Runs multiple ILS phases with an exponential schedule, each seeding the next.
Early phases use fewer instances and a shorter cutoff to filter the space cheaply;
later phases refine the best region with the full budget.

| Flag | Default | Description |
|------|---------|-------------|
| `--id` | `false` | Enable iterative deepening |
| `--lambda-n F` | `0.5` | Instance-count growth factor per phase |
| `--lambda-c F` | `0.5` | Cutoff-time growth factor per phase |
| `--lambda-t F` | `0.5` | Per-phase timeout growth factor |

### Debug

| Flag | Default | Description |
|------|---------|-------------|
| `--debug` | `false` | Print new incumbents and scores to stderr |
| `--debug-log PATH` | *(off)* | Write debug output to a file (independent of `--debug`) |
| `--debug-wrapper` | `false` | Print every solver wrapper invocation |
| `--debug-solver` | `false` | Print every solver result line |

## Output

The best configuration found, printed as `-param1 val1 -param2 val2 …`
(active parameters only, alphabetically sorted):

```
-alpha 1.256 -ps 0.1 -rho 0.5 -wp 0.03
```

This format is directly compatible with Grackle's strategy parsing.
