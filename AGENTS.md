# RamParILS Repository Guide

## Scope

- Make all code and documentation changes in this repository.
- `../paramils` is reference-only. It contains the
  original Ruby ParamILS implementation, papers, diagrams, and historical
  project notes. Do not modify it.
- Preserve unrelated local changes in either repository.

## Project

RamParILS is a parallel Rust implementation of ParamILS. It is exposed as:

- the `ramparils` CLI: `ramparils run <scenario.yaml>` to tune, and
  `ramparils db <cache>` to export a result cache;
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
- `src/ils/`: BasicILS, FocusedILS, perturbation, local search, capping, and
  iterative deepening, split into `mod.rs` (the `run()` orchestration loop),
  `bls.rs` (parallel first-improvement descent + single-config evaluation),
  `futell.rs` (checkpoint-based early rejection, DONE.md),
  `logging.rs`, `counters.rs` and `deepening.rs`.
- `src/main.rs`: the CLI — `run` and `db` sub-commands, and all of the clap
  structure; tuning options come from the scenario.
- `src/db.rs`: read-only export of a `.dbcache` (`solved`, `status`, `confs`).
- `src/lib.rs`: crate root. Signal handling, the elapsed-time clock, debug and
  crash logging, and the `GIT_REVISION` / `BUILD_INFO` constants.
- `src/python.rs`: optional PyO3 bindings.
- `build.rs`: stamps the git revision and build profile into those constants,
  with `rerun-if-changed` on `.git/HEAD` and `.git/index`. A binary that
  reports the wrong revision is worse than one that reports none, so treat
  this as behaviour, not build glue.
- `tests/`: integration tests; module-level unit tests live beside the code.
- `docs/figures/`: each figure keeps its source beside it and the command to
  regenerate it in that source's header.

Keep shared behavior in the library modules. The CLI and Python interface
should remain thin adapters over the same `Scenario` and ILS implementation.

## Commands

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
cargo build --release
maturin develop
./docs-build.sh          # ./docs-serve.sh to preview, ./docs-deploy.sh to publish
```

The E prover integration test may require external executables and can be
slower than unit tests. State explicitly when it was not run.

The tree is rustfmt-clean and `cargo fmt --check` passes; `rustfmt.toml` sets
`max_width = 120` and explains why. It was hand-formatted until v0.2.0, so any
commit before that will show a large diff against rustfmt — that is history,
not a regression.

`pip-build.sh` and `pip-upload.sh` build and publish the Python wheel;
`devel-reinstall.sh` reinstalls it locally. Publishing is a deliberate act —
run them only when asked.

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

- Rust edition: 2024; MSRV 1.85, declared as `rust-version` in `Cargo.toml`
  so cargo enforces it rather than the docs merely claiming it.
- Python: 3.9+ through the `python` Cargo feature.
- Keep `Cargo.toml` and `pyproject.toml` versions synchronized.
- Cutting a release touches five places, and the last two are the ones that
  rot: `Cargo.toml`, `pyproject.toml`, the `CHANGELOG.md` heading and its link
  block, the `--version` examples in `docs/usage/cli.md`, and the **current
  release line, install one-liner and "What's new" section in `docs/index.md`**,
  which is the published landing page. Tags are annotated, `RamParILS vX.Y.Z`,
  on the `chore: release` commit.
- Preserve the documented parameter-file syntax and solver wrapper protocol.
- Scenario fields are the single source of truth for tuning options.
- SQLite schema changes must include a migration path for existing caches.
- Cache access stays coordinated by the ILS side; solver workers should not
  independently mutate the SQLite connection.

## Example Scenario Naming

New `ramparils run` examples (not retroactive — `examples/primo` and other
existing examples keep their current names): scenario file `<name>.yaml`, no
`scenario-` prefix (e.g. `eprover-basic.yaml`); debug log `<name>.log`; error
log `<name>.errors`; cache `<name>.dbcache` (unless deliberately shared with
another scenario — e.g. `eprover-basic.yaml`, `eprover-random.yaml` and
`eprover-focused.yaml`, three approaches over the same algo/paramfile/
instances, all sharing `eprover.dbcache`). `params-`/`instances-` files keep
their existing prefixes, named after the benchmark/domain rather than the
scenario. Default naming — deviate only when asked.

## Function Structure

For a function that's the entry point to a nontrivial algorithm (the kind
worth calling out as "the main algorithm" in review), follow
`work26/plots/cactus/AGENTS.md`'s Python convention, carried over into Rust:
the function holds only the pipeline itself — one call per stage, no inline
logic — with each block factored into its own named function or method.
Applies to new code from here on; existing functions get this treatment
opportunistically when they're touched for another reason, not as a standing
mass-refactor mandate. `src/ils/bls.rs`'s `basic_local_search`/`collect_one`
and `src/ils/mod.rs`'s `run` are the worked examples to copy from (refactored
to this shape 2026-09-13 — read them alongside this section, not instead of
it).

**Folding many arguments — the Rust analog of Python's `*state` tuple
threading.** Rust has no positional splat, but the same underlying problem
(a stage needs to hand many related values to the next one, or a callee
needs many values from its caller) has two cleaner idioms, both already used
in this codebase:

- **A bundle passed by `&mut` reference**, when a group of values is
  constant across many calls in the same scope. `EvalContext<'a>` in
  `src/ils/bls.rs` bundles `scheduler`, `cache`, `options`, `space`,
  `cutoff_time` and `deadline` so `evaluate_config_outcome`/`collect_one`/
  `basic_local_search` take 5–7 arguments instead of 10+. Build it once at
  the top of the caller and thread `&mut ctx` through; the caller keeps its
  own `options`/`space` bindings for anything it still needs directly,
  rather than routing every access through `ctx.*` too.
- **A named struct with methods, for state carried across iterations.**
  `RunState` in `src/ils/mod.rs` replaces a ~10-variable loop (`incumbent`,
  `incumbent_score`, `last_lm`, `rejections`, `n_rounds`, ...) with one
  struct and named methods (`try_promote_incumbent`, `set_home_base`,
  `accept_or_reject_home_base`, `record_round`, `restart_reason`). This is
  the direct analog of a Python stage returning `state` for the next call as
  `next_stage(*state)` — except each field has a name instead of a
  position, and each transition is a method whose `&mut self` signature
  documents exactly which fields it touches, instead of one opaque tuple
  passed everywhere.
- Prefer the named-struct-with-methods form over a plain tuple once a group
  of values outlives a single call: a `(Config, ConfigEvaluation, usize)`
  return type is fine when it's consumed once at the call site, but a value
  group that survives more than one function call, or gets destructured in
  more than one place, is worth naming.

Don't retrofit this onto something that's already simple — a function with
one clear job and few branches doesn't need splitting just to have more
functions.

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
- Update README, `docs/` and `CHANGELOG.md` when changing public CLI, Python,
  scenario, parameter, cache, or wrapper behavior. Changelog entries go under
  `## [Unreleased]`; the published site includes that file, so an entry is
  visible before it is released.
