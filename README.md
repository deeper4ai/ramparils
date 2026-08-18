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
| Candidate evaluation | Sequential | **Parallel** over all `(neighbour, instance)` pairs |
| Result cache | In-memory, per run | **Persistent SQLite**, shared across runs, self-describing |
| Cache inspection | — | `ramparils db solved \| status \| confs` |
| Integration | Command line | Command line and **native Python extension** (PyO3) |
| Search modes | BasicILS, FocusedILS | BasicILS, FocusedILS, `random` (ParamILS's `pert_rand`) |
| Escaping a local optimum | Random restart at fixed probability (`p_restart`), `R` random probes | Both, **plus soft acceptance within a tolerance and stagnation-triggered restarts** |
| Restart target | Uniformly random configuration | Random, **or a bounded perturbation of the incumbent** |
| Comparing across fidelities | Score vector per configuration, compared at a common level | **Single score, re-measured** for the incumbent and the home base at every fidelity increase |
| Multi-phase schedules | — | **Iterative deepening**: geometric growth of instances, cutoff and deadline |
| Adaptive capping | Yes | Yes, with the ceiling rule documented — see [Algorithm](docs/reference/algorithm.md) |
| Provenance | — | Source revision in `--version` and in every log header; full scenario echoed at startup |
| Multiple random seeds | Supported | Not currently supported |

Parallel evaluation is the primary motivation for the rewrite. The actual
speedup depends on worker count, neighbourhood width, current fidelity, solver
runtimes, early acceptance, and cache hits.

RamParILS currently assumes that the target algorithm is deterministic. Cache
entries are keyed only by the active configuration and instance path. The
algorithm command, cutoff, objective, solver version, and random seed are not
part of the key. Use a separate cache whenever any of those change.

### 🖼️ What a run looks like

`approach: basic` — the variant that evaluates every candidate on the whole
instance set, so all scores are directly comparable. `focused` wraps the same
loop in a growing instance prefix; see [Algorithm](docs/reference/algorithm.md).

<p align="center">
  <img src="docs/figures/basic-ils.svg" width="100%"
       alt="Basic ILS: initialization, first local search, and the main loop">
</p>

Three configurations are in play at once, and keeping them apart is most of
understanding the search: **θ** the current candidate, **θ_base** the point
each perturbation starts from, and **θ_inc** the incumbent — the best seen, and
what the run returns. Only θ_base is perturbed, so once it stops moving the
search degenerates into repeated sampling from a fixed ball no matter what the
incumbent does. That is what `acceptance_tolerance` and the restart triggers
above exist to prevent.

## 📋 Requirements

- Rust 1.85 or newer for the CLI
- Python 3.9 or newer for the Python extension
- A Unix-like system for solver process-group and signal handling
- A target algorithm or wrapper that implements the RamParILS result protocol

## ⚙️ Installation

### 🦀 Command-line tools

Build the `ramparils` binary with Cargo:

```sh
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
cargo build --release
```

The executables are written to:

```text
target/release/ramparils
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
ramparils run scenario.yaml
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
| `perturbation_strength` | Parameters changed per perturbation; ParamILS's rule is `max(2, ceil(0.2 × parameters))` |
| `acceptance_tolerance` | Accept a home base within this relative margin of the incumbent; `0` keeps the ParamILS rule |
| `restart_failures` / `restart_probability` | Restart after *k* rejected local optima, or with a fixed probability per round |
| `restart_target` / `restart_strength` | Where a restart lands, and how far it jumps |
| `cores` | Worker count; `0` selects all available cores |
| `cache_db` | SQLite cache path; defaults to `:memory:` |

Useful optional controls include iterative deepening, initial evaluation
fidelity, adaptive capping, random probes, debug traces, and separate crash
logs. They are all covered in the
[CLI scenario reference](docs/usage/cli.md).

## 🗄️ Exporting the cache

`ramparils db` writes out what an existing cache holds, without running the
tuner. All three sub-commands produce one file per strategy hash, named
`ram-<hash>`, under a layout that mirrors solverpy's database — so an export
can be dropped straight into an existing `solverpy_db/`:

```sh
# All three at once.
ramparils db results.dbcache

# Or one at a time:

# Instances each strategy solved, one path per line.
ramparils db solved results.dbcache      # -> solverpy_db/solved/results/ram-<hash>

# Every result, as instance <TAB> status <TAB> runtime.
ramparils db status results.dbcache      # -> solverpy_db/status/results/ram-<hash>

# The configuration behind each hash, as YAML.
ramparils db confs results.dbcache       # -> solverpy_db/confs/results/ram-<hash>
ramparils db confs results.dbcache --json
```

`--out-dir` moves the root; it defaults to `solverpy_db`. Each command prints a
one-line summary on stdout and uses stderr for errors only.

Results are keyed by a hash of the active configuration, and the cache records
what each hash means in a `strategies` table, written the first time a
configuration is evaluated. Without it a `.dbcache` is a pile of opaque hashes:
recovering them would depend on the parameter space still being small enough to
enumerate and on the hash being reproducible by the current toolchain — and
`hash_config` uses `DefaultHasher`, which is explicitly not portable across
compiler versions.

Opening a cache written before the table existed adds it automatically;
existing results stay usable, but their hashes are only described from that
point on. `ramparils db confs` says so rather than writing an empty directory.

**`confs/` is not solverpy's `strats/`.** A strategy file there holds a solver
command line; a conf file here holds a parameter assignment, which only means
anything against the parameter space it was tuned in. It also records the
*active* configuration — parameters whose guard was closed are absent, since
that is exactly what the cache keys on — so it is a record of what ran rather
than a complete configuration, and `initial_config_file` will reject it unless
every parameter happened to be active.

The cache is most valuable when several tuning runs use exactly the same
solver semantics. Keep separate cache files when the algorithm command,
cutoff, objective, solver version, wrapper behavior, or benchmark meaning
changes.

## 🧪 Examples

- [`examples/primo`](examples/primo/README.md): tune the `primo` QF_LRA SMT
  solver. The most developed example — a 24-parameter conditional space with
  the evidence for each choice recorded inline, a BasicILS scenario whose knobs
  come from a nine-run campaign, and a header comment on what to adjust first
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

- [Changelog](CHANGELOG.md)
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
