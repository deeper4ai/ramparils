# 🐏 RamParILS

A parallel Rust rewrite of [ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/) —
automated algorithm configuration via Iterated Local Search.

Used as the inner tuner in [Grackle](https://github.com/ai4reason/grackle), a strategy portfolio
invention system for automated reasoning solvers.

**Current release: v0.3.0** (2026-10-29) · [changelog](changelog.md) ·
[sources and issues on GitHub](https://github.com/deeper4ai/ramparils) ·
[installation](installation.md)

```bash
pip install ramparils                                          # Python extension
cargo install --git https://github.com/deeper4ai/ramparils --tag v0.3.0 --locked   # CLI
```

## 💡 What it does

Given a target algorithm with configurable parameters, RamParILS searches for the parameter
setting that minimises runtime or a numeric solution cost on a set of training instances.
It uses **FocusedILS** by default. RamParILS starts by scoring configurations on a configurable
prefix of the training instances and increases that shared fidelity when the incumbent survives a
challenge. Neighbours are submitted to a bounded worker pool, and the first fully evaluated
improvement is accepted. Results can be stored in a **persistent SQLite cache** for reuse by
compatible tuning runs.

Cache entries are keyed only by the active configuration and instance path. Use a separate
cache when the algorithm command, cutoff, objective, solver version, wrapper behavior, or random
seed changes. Reusing a cache across incompatible scenarios can silently return stale results.

## 📦 What's new in 0.3.0

- Exact adaptive capping
- Wrapper `--version` check at startup (wrappers must implement it)
- `runhash` fingerprints and `ramparils db runhashes`
- `futell`: opt-in checkpoint-based early rejection
- `instance_shuffle`, on by default
- `cache_disable` scenario option
- Fixes in acceptance, logging and the example wrappers
- Agent skills for running a tuning campaign

Details for this and earlier releases are in the [changelog](changelog.md).

## 🤖 LLM skills

The repository ships skills for LLM coding agents such as Claude Code, which
walk an agent through a whole tuning campaign: building and verifying the
parameter space, writing the wrapper, setting up the scenario, running and
analysing RamParILS, and scoring the strategies found on a test benchmark
with SolverPy — with an experiment diary and predictions written before each
run. Copy [`skills/`](https://github.com/deeper4ai/ramparils/tree/main/skills)
into the agent's skills directory (`~/.claude/skills/` for Claude Code) and
ask it, in plain words, to tune your solver.
See [LLM skills](usage/skills.md) for the list, installation and usage.

## 🚀 Key differences from Ruby ParamILS

| | Original Ruby ParamILS | RamParILS |
|---|---|---|
| Candidate evaluation | Sequential (a cluster mode exists, tied to the authors' own database and hosts) | **Parallel** over all `(neighbour, instance)` pairs |
| Result cache | In memory, per run | **Persistent SQLite**, shared across runs, self-describing |
| Cache inspection | — | `ramparils db solved \| status \| confs \| runhashes` |
| Integration | Command line | Command line and **native Python extension** (PyO3) |
| Search modes | BasicILS, FocusedILS, pure random search, and `pert_rand` (a random configuration instead of a perturbation) | BasicILS, FocusedILS, `random` (ParamILS's `pert_rand`); no pure random search |
| Escaping a local optimum | Random restart at fixed probability (`p_restart`, default 0.01), `R` random probes (default 10) | Both (off by default), **plus soft acceptance within a tolerance and stagnation-triggered restarts** |
| Restart target | Uniformly random configuration | Random, **or a bounded perturbation of the incumbent** |
| Comparing across fidelities | Score vector per configuration, compared at a common level | **Single score, re-measured** for the incumbent and the home base at every fidelity increase |
| Iterative deepening | Yes: geometric growth of instances, cutoff and budget (`lambda_n`, `lambda_c`, `lambda_t`) | The same schedule; **a cached longer-cutoff result also answers a shorter-cutoff phase** |
| Adaptive capping | Yes (`boundMultiplier`, default 2) | Yes, **exact** cumulative-sum test, mean only — see [Algorithm](reference/algorithm.md) |
| Tuning budget | Summed target-algorithm runtime, maximum evaluations or iterations, or a target quality | Wall clock (`tuner_timeout`) |
| Test-set validation | Final configuration re-scored on test instances (`-validN`) | Not yet (`test_instance_file` is reserved); score separately, e.g. with SolverPy |
| Provenance | — | Source revision in `--version` and in every log header; full scenario echoed at startup |
| Multiple random seeds | Supported (instance–seed files) | Not currently supported |

Parallel evaluation is the primary motivation for the rewrite. Actual speedup depends on worker
count, neighbourhood width, current fidelity, solver runtimes, early acceptance, and cache hits.
The persistent cache compounds this advantage across compatible Grackle tuning runs on overlapping
problem sets.

<p align="center">
  <img src="figures/basic-ils.svg" width="100%"
       alt="Basic ILS: initialization, first local search, and the main loop">
</p>

The search in its `basic` form: θ is the current candidate, θ_base the point each perturbation
starts from, and θ_inc the incumbent that the run returns. See [Algorithm](reference/algorithm.md)
for what each box does and for the FocusedILS fidelity schedule.

## 🔗 Quick links

- [Installation](installation.md)
- [CLI usage](usage/cli.md)
- [Python API](usage/python.md)
- [Algorithm](reference/algorithm.md)
- [Parameter file format](reference/params.md)
- [Solver wrapper protocol](reference/protocol.md)
- [LLM skills](usage/skills.md)
- [Changelog](changelog.md)
- [GitHub repository](https://github.com/deeper4ai/ramparils)

## 🤝 Acknowledgements

This project is part of [DEEPER](https://deeper4ai.github.io/) and supported by the [DEEPER grant](https://www.renaissancephilanthropy.org/deeper-exploratory-engine-for-precise-expert-reasoning) from Renaissance Philanthropy.
