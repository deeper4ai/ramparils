# 🐏 RamParILS

> Give it a parameter space, a set of instances, and a time budget. Get back a
> faster or lower-cost algorithm configuration.

RamParILS is a parallel Rust implementation of
[ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/), an
automatic algorithm configurator based on Iterated Local Search. It searches
discrete parameter spaces, evaluates candidate configurations concurrently,
and can keep every result in a persistent SQLite cache.

RamParILS is used as the inner tuner in
[Grackle](https://github.com/ai4reason/grackle), but it is designed to work
with any command-line algorithm that can follow the small wrapper protocol
described below.

## 💡 The idea

Many solvers and heuristic programs expose dozens of options. Choosing them by
hand is slow, benchmark-specific, and often surprising. RamParILS automates
that work:

1. You describe the legal parameter values in a ParamILS-compatible file.
2. You list representative training instances.
3. A wrapper runs your target algorithm and reports runtime or solution
   quality.
4. RamParILS explores configurations until the tuning budget expires and
   prints the best one found.

FocusedILS is the default search method. Promising configurations are tested on
more instances while poor candidates are rejected early. Independent solver
runs are distributed across a bounded worker pool, and adaptive capping avoids
spending the full cutoff on configurations that are already losing.

## 🚀 Why RamParILS?

| | Original Ruby ParamILS | RamParILS |
|---|---|---|
| Candidate evaluation | Sequential | Parallel |
| Result cache | In-memory, per run | SQLite, optionally persistent |
| Integration | Command line | Command line and native Python extension |
| Search modes | BasicILS, FocusedILS | BasicILS, FocusedILS, `random` compatibility mode |
| Multiple random seeds | Supported | Not currently supported |

RamParILS currently assumes that the target algorithm is deterministic. Cache
entries are keyed only by the active configuration and instance path. The
algorithm command, cutoff, objective, solver version, and random seed are not
part of the key. Use a separate cache whenever any of those change.

## 📋 Requirements

- Rust 1.85 or newer for the CLI
- Python 3.9 or newer for the Python extension
- A Unix-like system for solver process-group and signal handling
- A target algorithm or wrapper that implements the RamParILS result protocol

## ⚙️ Installation

### 🦀 Command-line tools

Build the `ramparils` tuner and the `ramparils-db` cache inspector with Cargo:

```sh
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
cargo build --release
```

The executables are written to:

```text
target/release/ramparils
target/release/ramparils-db
```

Install Rust with [rustup](https://rustup.rs/) if `cargo` is not available.

### 🐍 Python extension

Install the published package:

```sh
python -m pip install ramparils
```

For local development, create or activate a virtual environment and run:

```sh
python -m pip install maturin
maturin develop
```

See the full [installation guide](docs/installation.md) for more detail.

## ⚡ Quick start

A tuning job needs four small pieces.

### 1️⃣ Describe the parameters

Create `solver.params`:

```text
# name   {allowed values}       [default]
engine   {quick, thorough}       [quick]
threads  {1, 2, 4, 8}            [4]
restart  {none, geometric}        [geometric]
factor   {1.2, 1.5, 2.0}          [1.5] | restart in {geometric}

# This combination must never be evaluated.
{engine=thorough, threads=1}
```

Values are passed as strings. Parameters may be conditional, and forbidden
combinations are excluded from the search. The syntax is documented in
[Parameter files](docs/reference/params.md).

### 2️⃣ List training instances

Create `instances.txt`, with one path per line:

```text
benchmarks/easy-01.in
benchmarks/easy-02.in
benchmarks/hard-01.in
```

Blank lines and lines beginning with `#` are ignored. Paths are interpreted by
the wrapper, so running from a predictable working directory is recommended.

### 3️⃣ Write a solver wrapper

RamParILS invokes the configured command as:

```text
<algo> <instance> <cutoff_time> -parameter value ...
```

The wrapper may print normal diagnostic output, but it must include one result
line on stdout:

```text
#%# RamParIls #%# <status>, <runtime>, <quality>
```

For example:

```text
#%# RamParIls #%# OK, 1.237, 0.0
```

`status` is preserved as text, `runtime` is measured in seconds, and `quality`
is used for quality-oriented scenarios. Missing or malformed results are
penalized. See the [wrapper protocol](docs/reference/protocol.md) for a minimal
wrapper and failure behavior.

### 4️⃣ Create and run a scenario

Create `scenario.yaml`:

```yaml
algo: "python3 solver_wrapper.py"
paramfile: "solver.params"
instance_file: "instances.txt"

cutoff_time: 10.0
tuner_timeout: 300.0
run_obj: runtime
overall_obj: mean

approach: focused
cores: 0                    # 0 uses all available CPU cores
cache_db: "results.dbcache" # reuse evaluations in later runs
debug: true
```

Then run:

```sh
ramparils --scenariofile scenario.yaml
```

The complete best configuration is printed to stdout as YAML:

```yaml
engine: quick
factor: '1.2'
restart: geometric
threads: '8'
```

All tuning options live in the scenario file, keeping runs reproducible and
easy to share. The [CLI guide](docs/usage/cli.md) lists every field and its
default.

## 🐍 Python API

The Python extension runs the same Rust implementation in-process. Pass an
initial strategy and a scenario dictionary:

```python
import ramparils

best = ramparils.specialize(
    strategy={
        "engine": "quick",
        "threads": "4",
        "restart": "geometric",
        "factor": "1.5",
    },
    scenario={
        "algo": "python3 solver_wrapper.py",
        "paramfile": "solver.params",
        "instances": [
            "benchmarks/easy-01.in",
            "benchmarks/easy-02.in",
            "benchmarks/hard-01.in",
        ],
        "cutoff_time": 10.0,
        "tuner_timeout": 300.0,
        "cores": 0,
        "cache_db": "results.dbcache",
    },
)

print(best)
```

The result is a `dict[str, str]` containing the best active parameters. The
initial strategy must provide every parameter from the parameter file,
including parameters that may become inactive. See the
[Python API guide](docs/usage/python.md) and
[`examples/saps_python.py`](examples/saps_python.py).

## 🧭 Scenario essentials

| Field | Purpose |
|---|---|
| `algo` | Shell command for the target algorithm or wrapper |
| `paramfile` | ParamILS-compatible parameter-space file |
| `initial_config` / `initial_config_file` | Optional complete ILS starting configuration, inline or in a YAML file |
| `instance_file` / `instances` | Training instances; use exactly one |
| `cutoff_time` | Per-solver-run limit in seconds |
| `tuner_timeout` | Total tuning budget in seconds |
| `run_obj` | Minimize `runtime` or the numeric `quality` cost |
| `overall_obj` | Aggregate results with `mean` or `median` |
| `approach` | `focused`, `basic`, or `random` |
| `cores` | Worker count; `0` selects all available cores |
| `cache_db` | SQLite cache path; defaults to `:memory:` |

Useful optional controls include iterative deepening, initial evaluation
fidelity, adaptive capping, debug traces, and separate crash logs. They are all
covered in the [CLI scenario reference](docs/usage/cli.md).

## 🗄️ Inspecting the cache

`ramparils-db` exports per-strategy summaries from an existing cache without
running the tuner:

```sh
# Files containing the solved instance names for each strategy.
ramparils-db solved results.dbcache --out-dir reports

# Tab-separated instance, status, and runtime tables.
ramparils-db status results.dbcache --out-dir reports
```

The cache is most valuable when several tuning runs use exactly the same
solver semantics. Keep separate cache files when the algorithm command,
cutoff, objective, solver version, wrapper behavior, or benchmark meaning
changes.

## 🧪 Examples

- [`examples/llm2smt`](examples/llm2smt/README.md): tune an SMT solver with a
  standalone Python wrapper
- [`examples/eprover`](examples/eprover): specialize E prover strategies for
  Grackle
- [`examples/saps_python.py`](examples/saps_python.py): call RamParILS through
  Python

## 🛠️ Development

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
cargo build --release
./docs-build.sh
```

The E prover integration test requires external executables and may be slower
than the unit tests.

Preview the documentation with `./docs-serve.sh`. Maintainers can publish a
clean, committed tree to
[GitHub Pages](https://deeper4ai.github.io/ramparils/) with
`./docs-deploy.sh`.

## 📚 Documentation

- [Installation](docs/installation.md)
- [CLI and scenario files](docs/usage/cli.md)
- [Python API](docs/usage/python.md)
- [Search algorithm](docs/reference/algorithm.md)
- [Parameter-file syntax](docs/reference/params.md)
- [Solver wrapper protocol](docs/reference/protocol.md)
- [Glossary](docs/reference/glossary.md)

## 🤝 Acknowledgements

RamParILS is part of [DEEPER](https://deeper4ai.github.io/) and is supported by
the [DEEPER grant](https://www.renaissancephilanthropy.org/deeper-exploratory-engine-for-precise-expert-reasoning)
from Renaissance Philanthropy.
