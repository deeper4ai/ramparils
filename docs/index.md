# RamParILS

A parallel Rust rewrite of [ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/) —
automated algorithm configuration via Iterated Local Search.

Used as the inner tuner in [Grackle](https://github.com/ai4reason/grackle), a strategy portfolio
invention system for automated reasoning solvers.

## What it does

Given a target algorithm with configurable parameters, RamParILS searches for the parameter
setting that minimises runtime (or maximises solution quality) on a set of training instances.
It uses **FocusedILS** by default — an adaptive variant of Iterated Local Search that compares
configurations after each solver run rather than after a fixed number, so poor configurations are
filtered in one or two calls while promising ones receive the full evaluation budget.
Each local search step evaluates all neighbours of the current configuration **in parallel** across
all available CPU cores, and results are stored in a **persistent SQLite cache** so that repeated
runs on the same benchmark never redo solver calls.

## Key differences from Ruby ParamILS

|                | Ruby ParamILS                | RamParILS                                      |
|----------------|------------------------------|------------------------------------------------|
| Evaluation     | Sequential                   | Parallel over all `(neighbor, instance)` pairs |
| Cache          | In-memory, per-run           | Persistent SQLite, shared across runs          |
| Python API     | subprocess call              | Native extension via PyO3                      |
| Non-deterministic algorithms (multiple seeds) | Supported | Not (yet) supported |

The parallel evaluation is the primary motivation for the rewrite: on a machine with 60 cores,
a single local search step that would take 60 × `cutoff_time` seconds sequentially completes in
roughly `cutoff_time` seconds.  The persistent cache compounds this advantage across Grackle's
many short tuning runs on overlapping problem sets — a strategy evaluated in one run is never
re-evaluated in another.

## Quick links

- [Installation](installation.md)
- [CLI usage](usage/cli.md)
- [Python API](usage/python.md)
- [Algorithm](reference/algorithm.md)
- [Parameter file format](reference/params.md)
- [Solver wrapper protocol](reference/protocol.md)

## Acknowledgements

This project is part of [DEEPER](https://deeper4ai.github.io/) and supported by the [DEEPER grant](https://www.renaissancephilanthropy.org/deeper-exploratory-engine-for-precise-expert-reasoning) from Renaissance Philanthropy.
