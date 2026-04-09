# CLI

```bash
parils --scenariofile path/to/scenario.yaml
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

| Flag | Default | Description |
|------|---------|-------------|
| `--scenariofile PATH` | *(required)* | Scenario YAML file |
| `--cores N` | all cores | Parallel worker threads (`0` = auto) |
| `--approach` | `focused` | `basic` \| `focused` \| `random` |
| `--N N` | `10` | Max runs per config per detail level |
| `--bm F` | `10.0` | Adaptive capping bound multiplier |
| `--ps N` | `4` | Perturbation strength |
| `--pruning` | `true` | Enable adaptive capping |
| `--cachedb PATH` | `paramils_cache.db` | SQLite cache file |
| `--numRun N` | `0` | Run index (reserved, future random seed) |

## Output

The best configuration found, printed as `-param1 val1 -param2 val2 …`
(active parameters only, alphabetically sorted):

```
-alpha 1.256 -ps 0.1 -rho 0.5 -wp 0.03
```

This format is directly compatible with Grackle's strategy parsing.
