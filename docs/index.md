# RamParILS

A parallel Rust rewrite of [ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/) —
automated algorithm configuration via Iterated Local Search.

Used as the inner tuner in [Grackle](https://github.com/ai4reason/grackle), a strategy portfolio
invention system for automated reasoning solvers.

## What it does

Given a target algorithm with configurable parameters, RamParILS finds the parameter setting that
minimises runtime (or maximises solution quality) on a set of training instances.

It evaluates `(configuration, instance)` pairs **in parallel** across all available CPU cores and
caches results in a persistent SQLite database — repeated runs on the same benchmark never redo
solver calls.

## Key differences from Ruby ParamILS

|                | Ruby ParamILS                | RamParILS                                      |
|----------------|------------------------------|---------------------------------------------|
| Evaluation     | Sequential                   | Parallel over all `(neighbor, instance)` pairs |
| Cache          | In-memory, per-run           | Persistent SQLite, shared across runs       |
| Python API     | subprocess call              | Native extension via PyO3                   |
| Non-deterministic algorithms (multiple seeds) | Supported | Not (yet) supported |

## Quick links

- [Installation](installation.md)
- [CLI usage](usage/cli.md)
- [Python API](usage/python.md)
- [Parameter file format](reference/params.md)
- [Solver wrapper protocol](reference/protocol.md)
