# RamParILS

A parallel Rust rewrite of [ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/) — automated algorithm configuration via Iterated Local Search.

Used as the inner tuner in [Grackle](https://github.com/ai4reason/grackle), a strategy portfolio invention system for automated reasoning solvers.

## What it does

Given a target algorithm with configurable parameters, RamParILS finds the parameter setting that minimises runtime (or maximises solution quality) on a set of training instances.
It evaluates `(configuration, instance)` pairs in parallel across all available CPU cores and caches results in a persistent SQLite database so repeated runs on the same benchmark never redo solver calls.

## Key differences from the Ruby ParamILS

| | Ruby ParamILS | RamParILS |
|---|---|---|
| Evaluation | Sequential | Parallel over all `(neighbor, instance)` pairs |
| Cache | In-memory, per-run | Persistent SQLite, shared across runs |
| Python API | subprocess call | Native extension via PyO3 |
| Non-deterministic algorithms (multiple seeds) | Supported | Not (yet) supported |

---

## Installation

### Python extension (pip)

```bash
pip install ramparils
```

### CLI binary

Clone the repo and build with Cargo ([Rust 1.85+](https://rustup.rs) required):

```bash
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
cargo build --release
# binary at target/release/ramparils
```

---

## Usage

### CLI

```bash
ramparils --scenariofile path/to/scenario.yaml
```

**Scenario file** (`scenario.yaml`):

```yaml
algo: "ruby /path/to/solver_wrapper.rb"      # command to invoke the target algorithm
paramfile: "/path/to/params.txt"             # parameter space definition
instance_file: "example_data/train.txt"     # one instance path per line
cutoff_time: 5.0                             # per-run time limit (seconds)
tuner_timeout: 300.0                         # total wall-clock budget (seconds)
run_obj: runtime                             # runtime | quality
overall_obj: mean                            # mean | median
```

**Key flags:**

| Flag | Default | Description |
|------|---------|-------------|
| `--cores N` | all cores | parallel worker threads |
| `--approach` | `focused` | `basic` \| `focused` \| `random` |
| `--bm F` | `10.0` | adaptive capping bound multiplier |
| `--ps N` | `4` | perturbation strength |
| `--cachedb PATH` | `paramils_cache.db` | SQLite cache file |

**Output:** the best configuration found, printed as `-param1 val1 -param2 val2 …` (active parameters only, alphabetically sorted).

### Python

```python
import ramparils

result = ramparils.specialize(
    strategy={"alpha": "1.189", "rho": "0.5", "ps": "0.1", "wp": "0.03"},
    scenario={
        "algo":         "ruby /path/to/solver_wrapper.rb",
        "paramfile":    "/path/to/params.txt",
        # Pass instances as a list:
        "instances":    ["inst1.cnf", "inst2.cnf", "inst3.cnf"],
        # Or as a file path:
        # "instance_file": "example_data/train.txt",
        "cutoff_time":  5.0,
        "tuner_timeout": 300.0,
    },
    cache_db="results.db",
    cores=60,
)
# result is a dict[str, str] — the improved strategy
print(result)
```

`specialize()` uses FocusedILS with sensible defaults and starts from the provided `strategy`. See `examples/saps_python.py` for a runnable example.

---

## Parameter file format

```
# name  {domain}  [default]
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189]
rho   {0, 0.17, 0.5, 1} [0.5]

# Conditional: child only active when parent has a specific value
child {a, b} [a] | parent in {val1, val2}

# Forbidden combination
{alpha=1.01, rho=0}
```

---

## Solver wrapper protocol

The target algorithm is invoked as:

```
<algo> <instance> <cutoff_time> -param1 val1 -param2 val2 …
```

It must print a result line to stdout:

```
#%# RamParIls #%# <OK|TIMEOUT|CRASHED>, <runtime>, <quality>
```

See `examples/saps_python.py` for a runnable example.

---

## Testing

```bash
cargo test
```

25 unit tests covering the parameter space parser, SQLite cache, scheduler, and ILS core functions.

## Acknowledgements

This project is part of [DEEPER](https://deeper4ai.github.io/) and supported by the [DEEPER grant](https://www.renaissancephilanthropy.org/deeper-exploratory-engine-for-precise-expert-reasoning) from Renaissance Philanthropy.
