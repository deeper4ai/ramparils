# RamParILS Repository Guide

## Scope

- Make all code and documentation changes in this repository.
- `../paramils` is reference-only. It contains the
  original Ruby ParamILS implementation, papers, diagrams, and historical
  project notes. Do not modify it.
- Preserve unrelated local changes in either repository.

## Project

RamParILS is a parallel Rust implementation of ParamILS. It is exposed as:

- the `ramparils` CLI;
- the `ramparils-db` cache inspection CLI;
- a Python extension built with PyO3 and maturin.

The primary use case is strategy specialization inside Grackle. Target
algorithms are currently assumed to be deterministic, so cache keys do not
include a random seed.

## Architecture

- `src/params.rs`: parameter-space parsing, defaults, conditionals, forbidden
  configurations, and the `Config` type.
- `src/scenario.rs`: YAML/Python scenario model and instance loading.
- `src/cache.rs`: persistent SQLite result cache.
- `src/eval.rs`: parallel solver execution and result parsing.
- `src/ils.rs`: BasicILS, FocusedILS, perturbation, local search, capping, and
  iterative deepening.
- `src/main.rs`: main CLI; tuning options come from the scenario.
- `src/python.rs`: optional PyO3 bindings.
- `src/bin/ramparils_db.rs`: cache inspection and export commands.
- `tests/`: integration tests; module-level unit tests live beside the code.

Keep shared behavior in the library modules. The CLI and Python interface
should remain thin adapters over the same `Scenario` and ILS implementation.

## Commands

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
cargo build --release
maturin develop
./docs-build.sh
```

The E prover integration test may require external executables and can be
slower than unit tests. State explicitly when it was not run.

## Commits

Use Conventional Commit-style subjects:

```text
<type>[optional scope]: <description>
```

Match recent repository history: use a concise subject and an explanatory body
for non-trivial changes. When an AI assistant materially authors a commit, add
the appropriate `Co-Authored-By` trailer for the assistant that actually did
the work. Do not falsely attribute work to another assistant.

## Compatibility

- Rust edition: 2024; minimum documented Rust version: 1.85.
- Python: 3.9+ through the `python` Cargo feature.
- Keep `Cargo.toml` and `pyproject.toml` versions synchronized.
- Preserve the documented parameter-file syntax and solver wrapper protocol.
- Scenario fields are the single source of truth for tuning options.
- SQLite schema changes must include a migration path for existing caches.
- Cache access stays coordinated by the ILS side; solver workers should not
  independently mutate the SQLite connection.

## Engineering Conventions

- Follow existing module patterns and use `anyhow::Result` for fallible
  application paths.
- Keep solver evaluation parallelism bounded by the configured worker count.
- Treat timeout, crash, quality, runtime, and solver status semantics as
  user-visible behavior.
- Add focused unit tests for parser, cache, and ILS logic changes. Add or
  update integration tests when behavior crosses module boundaries.
- Run formatting and the narrowest relevant tests after edits, then broaden
  verification according to the change's risk.
- Update README and `docs/` when changing public CLI, Python, scenario,
  parameter, cache, or wrapper behavior.
