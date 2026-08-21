# Changelog

All notable changes to RamParILS are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

Dates are commit dates. Entries were reconstructed from the git history, so
they describe what changed rather than what was announced at the time.

## [Unreleased]

### Added

- An end-of-run `ils: summary` line reporting
  `rounds / searched / gated / incumbents / evals / capped`. A *gated* round is
  one whose starting configuration was capped and which then accepted no move,
  so the bound hid its whole neighbourhood and it produced no search. Comparing
  two approaches on final score alone can hide that one of them was pruned out
  of most of its rounds.
- The changelog is published with the documentation, at
  `deeper4ai.github.io/ramparils`. `docs/changelog.md` is a one-line mdBook
  include of this file, so there is still one source of truth.
- The landing page carries the current release, an install one-liner pinned to
  the tag, a "What's new" section and a link to the GitHub repository.
- **A `--version` probe before the first evaluation.** `ramparils` now runs
  `<algo> --version` at startup and refuses to start unless it exits `0` and
  its last stdout line is `supports: … version …`; the wrapper's whole
  response is logged as its own block, separated from the run's other startup
  stats. This is the fix for a real incident: a 24 h tuning run launched
  against a wrapper with no solver binary on `PATH` and no instance files in
  place ran to completion reporting nothing wrong, because the wrapper
  answered every evaluation with a well-formed but meaningless result line
  instead of failing to launch. A wrapper that can't reach its solver prints a
  `<solver> MISSING` placeholder line in the version block but must still
  exit non-zero. `examples/primo` and `examples/eprover` both implement the
  convention; see docs/reference/protocol.md.
- **A `runhash` fingerprint, threaded end to end.** A wrapper's result line
  may carry an optional fourth field: a hash of the solver's own internal
  counters, independent of runtime, that lets two configurations be compared
  for having done byte-identical work. `results.runhash` is a new nullable
  cache column; a descent XORs it across every evaluated neighbour and logs
  it beside each incumbent/home base; `ramparils db status` exports it as a
  fourth column; and a new `ramparils db runhashes` writes
  `ram-<hash> <runhash> <n>` per strategy (the XOR of every non-null runhash
  for that hash, skipping instances with none rather than disqualifying the
  whole hash; `n` counts every attempted instance, so `n == instances` means
  fully evaluated). Given a cache and no sub-command, `ramparils db` now
  exports all four (`solved`, `status`, `confs`, `runhashes`). Two strategies
  sharing a runhash did identical internal work, which is the signal a
  structurally dead parameter needs and nothing else can catch.
- **`examples/eprover` rewritten as a grackle-free wrapper** via solverpy's
  `E`, replacing the old grackle-dependent example. A deliberately small
  domain — core proof-search switches, term ordering, and up to 4
  independently-tunable clause-selection heuristic slots with their own
  frequencies — rather than grackle's full combinatorial space. Three
  scenarios (`eprover-basic`/`-random`/`-focused`) share one cache, differing
  only in search approach and fidelity schedule.

### Changed

- **Adaptive capping now tests the cumulative sum against a budget** —
  `partial_sum > bound_multiplier × incumbent_score × n_instances` — instead of
  the running mean against `bound_multiplier × incumbent_score`. Costs never go
  down, so passing the budget proves the final mean exceeds the bound: capping
  becomes exact rather than heuristic and never discards a configuration that
  would have been accepted. It also fixes both ends of the old behaviour.
  Results arrive fastest-first, so the running mean was a lower bound that only
  converged at the end and most capped evaluations ran nearly the whole instance
  set; at the other extreme there was no minimum sample, and one instance above
  the bound capped a configuration outright. Now no cap is possible before
  `bound_multiplier × incumbent_score / cutoff_time` of the set. The meaning of
  `bound_multiplier` is unchanged, so no scenario file needs editing.
- A capped score is now logged as `>2.698475 (312/473)` rather than as a plain
  number. It is a mean over the instances that finished first — the fastest —
  so it understates the true score: the `>` marks it as a lower bound and the
  ratio says how much was actually seen, since a cap after 1 instance and a cap
  after 470 are not the same claim. Affects `ils: bls local optimum`,
  `ils: bls improvement … (was …)`, `ils: new home base` and
  `ils: restart: … score=`. Two capped scores cover different, differently
  biased prefixes and must not be compared with each other.
- Adaptive capping is logged under `debug` rather than `debug_wrapper`. It is
  one line per evaluation, not one per solver call, and the event explaining why
  a neighbourhood yielded no improvement was invisible in an ordinary debug log.
- **`examples/primo/primo_wrapper.py` migrated to solverpy's `Primo`**, which
  already supplies time/memory limits, SMT status parsing and the `runhash`
  fingerprint, replacing the wrapper's own hand-rolled subprocess/ulimit
  plumbing. Drops the `PRIMO` environment-variable override in favour of
  solverpy's own binary resolution — point a different build at `PATH` under
  the expected name instead. Also gains the `--version`/`supports:` protocol
  above and a `--params` dry-run flag that resolves a parameter set to a
  command line without running anything.

### Fixed

- **Debug and error logs no longer truncate on a rerun.** Both were opened
  with `File::create`, so a second `ramparils run` (or `specialize()` call)
  against the same paths silently discarded the previous run's history. They
  now open in append mode, so a rerun's output adds to the running history.
- **The error log is created lazily, on the first crash, not at startup.** It
  used to be created eagerly even when nothing ever crashed, so every clean
  run left a 0-byte file behind that looked exactly like "checked, nothing
  wrong" — indistinguishable from a real crash report that was never written.
- **A wrapper crash is routed through `UNKNOWN` with PAR1 scoring, not an
  invented status.** `examples/primo/primo_wrapper.py` used to report a crash
  as its own `"error"` status with the real (possibly near-instant) elapsed
  time; RamParILS doesn't recognise `"error"` as special, so every crash was
  silently cached as a legitimate result and the error log — the one place a
  human would notice — stayed empty, while a fast-failing configuration could
  score better than a genuine solve. It now reuses RamParILS's own `UNKNOWN`
  sentinel (logged, excluded from the cache) and always charges the full
  cutoff on any non-success line. Found via `examples/eprover`'s new wrapper,
  where the same two bugs let a batch of invalid parameter values silently
  score better than real solves on ~43% of evaluations; see
  docs/reference/protocol.md.

## [0.2.0] — 2026-08-19

The escape mechanism, the provenance stamping, a unified CLI and a reworked
set of documents.

**Upgrading**: the CLI is not compatible with 0.1.x. `ramparils run
<scenario.yaml>` replaces `ramparils --scenariofile <scenario.yaml>`, and the
`ramparils-db` binary is now `ramparils db`. Scenario files, parameter files,
caches and the Python API are unchanged, so only the invocation moves.

### Added

- **Escape mechanism for a frozen ILS home base.** The acceptance criterion
  only ever replaced the home base with an at-least-as-good local optimum, so
  nothing in the loop could move the search uphill: once a strong local optimum
  was found, every later round perturbed the same point. Five new scenario
  fields address it, all defaulting to previous behaviour:
  - `acceptance_tolerance` — accept a *worse* local optimum as the home base
    while it stays within this relative margin of the **incumbent** (measured
    against the incumbent, not the home base, so the margin cannot compound);
  - `restart_failures` — restart after this many consecutive rejected local
    optima, which adapts to however many rounds a budget turns out to allow;
  - `restart_probability` — ParamILS's `p_restart`;
  - `restart_target` (`incumbent` | `random`) and `restart_strength` — where a
    restart lands and how far it jumps. `0` resolves to
    `2 × perturbation_strength`, and the resolved value is printed in the
    debug header;
  - `random_probes` — ParamILS's `R`, previously unreachable because
    `resolve_initial_config` always returned a configuration. Defaults to `0`:
    specializing a caller-supplied strategy should start from that strategy.

  Restarts and home-base replacements are logged distinctly
  (`ils: restart:`, `ils: new home base:` with a parameter diff), so a run
  dragged along by its escape mechanism can be told from a healthy one.
- **Source revision in `--version` and in every debug-log header**, stamped at
  build time by `build.rs`. A `-dirty` suffix marks an unclean worktree, and a
  build without git reads `unknown` rather than failing. The version alone
  never identified the code, since a tag covers every commit after it.
- **`docs/figures/basic-ils.svg`**, a diagram of BasicILS shown in the README,
  the documentation index and the algorithm reference, with its regenerable
  TikZ source beside it.
- **A "Designing a space" section** in the parameter-file reference: what a
  domain costs in every neighbourhood, why declaring conditionals is free and
  their absence is not, conditionals versus forbidden combinations, and why a
  guard is only explorable if it pays at its dependents' default values.
- **`rust-version = "1.85"`** in `Cargo.toml`, matching the MSRV the
  documentation already claimed.
- **`rustfmt.toml`** (`max_width = 120`), and the whole tree reformatted to
  match. The code had been hand-formatted since the first commit, so
  `cargo fmt --check` — listed as a standard command in `AGENTS.md` and the
  README — had never passed. It passes now, and can be enforced in CI. 120
  rather than rustfmt's default 100 because the dominant pattern here is a
  `debug_line(d, &format!(…))` call written to read like the log line it
  produces; the file records the measurements behind the choice.
- **`CHANGELOG.md`**, this file.
- **`examples/primo`** gained the flattening and SOI-minimization options
  (`boolean_flatten_threshold`, `boolean_flatten_post_threshold`,
  `lra_soi_minimize`, `lra_soi_minimize_order`) in its wrapper, and a revised
  24-parameter space, `params-primo-qflra.txt`, carrying the measurement behind
  each choice. Its scenario now runs BasicILS with settings derived from a
  nine-run tuning campaign, and documents what to adjust first.

### Changed

- **BREAKING: one binary, two sub-commands.** `ramparils run <scenario.yaml>`
  replaces `ramparils --scenariofile <scenario.yaml>`, and the separate
  `ramparils-db` binary is gone — its sub-commands are now `ramparils db`.
  There is no compatibility shim: the old forms are errors.

  `db` also changes shape. All three sub-commands are exports now, writing one
  file per strategy hash named `ram-<hash>` under `--out-dir`, which defaults
  to `solverpy_db` rather than the current directory, in a layout that mirrors
  solverpy's database so an export can be dropped into an existing
  `solverpy_db/`. Each prints a one-line summary on stdout and uses stderr for
  errors only.

  - `solved` and `status` now record the **full instance path** the cache
    stored, not the basename, matching solverpy's files.
  - `strategies` is renamed **`confs`** and writes files rather than a table on
    stdout: one per hash, holding the configuration as YAML (`--json` for the
    stored JSON). It is deliberately not solverpy's `strats/` — that holds a
    solver command line, this holds a parameter assignment, which only means
    anything against the parameter space it was tuned in. Note it records the
    *active* configuration, so it is a record of what ran rather than a
    complete one, and `initial_config_file` will reject it unless every
    parameter was active.
  - `solved`'s success-status set is now documented in `--help`.
  - given a cache and no sub-command, `db` runs all three:
    `ramparils db results.dbcache` is `solved`, `status` and `confs` in one go.
- **A closed stdout no longer panics.** Rust ignores `SIGPIPE` at startup, so
  `println!` panicked with a backtrace when the reader went away — piping any
  of this into `head` did it, including the old `ramparils-db strategies`,
  whose table output existed to be piped. `main` now restores the default
  disposition, so the process exits quietly with 141 as any Unix tool does.
- **`approach: random` is now ParamILS's `pert_rand`** — a fresh random
  configuration each round with the acceptance criterion skipped, i.e. a
  random-restart baseline. It was previously a silent alias for `basic`, so any
  earlier run that set it was really running BasicILS.
- The adaptive-capping documentation now states the ceiling rule: under a PAR1
  runtime objective capping cannot fire unless
  `bound_multiplier × incumbent_score < cutoff_time`, so a multiplier just
  below that ratio is indistinguishable from `pruning: false`.
- `examples/primo/params-primo.txt` was renamed to `params-primo-qflra.txt`.

### Fixed

- `examples/eprover/run.sh` had been broken since 0.1.2: it passed `--debug`,
  `--debug-log` and `--cachedb`, which became scenario fields in that release,
  so the script could not have run. Those three settings moved into its
  `scenario.yaml`, where they belong.
- The SAPS example in the parameter-file reference did not parse: `wp`'s
  default `0.03` was absent from its domain, so anyone copying it hit
  `default '0.03' not in domain`.
- The Python API reference claimed `specialize` runs FocusedILS; it runs
  whichever variant `scenario["approach"]` selects.

## [0.1.3] — 2026-08-06

### Added

- **The cache records what each strategy hash means** (`strategies` table,
  written the first time a configuration is evaluated, and added automatically
  when an older cache is opened). Without it a `.dbcache` is a pile of opaque
  hashes whose recovery depends on the space still being small enough to
  enumerate and on `DefaultHasher` being reproducible across compiler versions —
  which it is explicitly not. Exposed as `ramparils-db strategies`.
- **Cutoff-aware result caching.** Each result stores the cutoff it was
  measured under: a timeout satisfies only requests with an equal or shorter
  cutoff, and a completed run exceeding a shorter requested cutoff is returned
  as an in-memory synthetic timeout and never written back. Caches predating
  this are incompatible and must be replaced.
- **Scenario initial configurations**, inline via `initial_config` or in a file
  via `initial_config_file`, validated against the parameter space.
- `examples/primo`, and `guarded_real_equality_lowering` in its space.
- Strategy extraction from tuning logs for the `llm2smt` example.

### Fixed

- **FocusedILS compared scores taken at different fidelities.** The incumbent
  was re-measured when the fidelity grew but the ILS home base was not, so the
  acceptance criterion compared a current score against a stale one taken on a
  shorter prefix. Because prefix means drift as the prefix grows, the stale bar
  was biased low and the only mechanism that could update it was the comparison
  it blocked — the home base froze for the rest of the run. Both retained
  states are now re-measured at every increase, and each increase is logged
  with both scores.
- An incomplete fidelity increase at the deadline no longer discards the
  incumbent's score.
- Canceled solver workers are terminated rather than left running; solver
  process trees are terminated on interrupt; queued solver work is bounded.

## [0.1.2] — 2026-06-10

### Added

- **All tuning knobs unified into the scenario file.** The CLI keeps only
  `--scenariofile` (and `--version`), which makes a run reproducible from one
  file.
- Configurable FocusedILS evaluation fidelity (`initial_fidelity`,
  `fidelity_step`).
- Iterative deepening (`iterative_deepening`, `lambda_n`, `lambda_c`,
  `lambda_t`): multiple ILS phases on an exponential schedule of instances,
  cutoff and cumulative deadline.
- `ramparils-db` with `solved` and `status` sub-commands.
- Structured debug logging: `debug`, `debug_log`, `debug_wrapper`,
  `debug_solver`, and `error_log` for crash reporting.
- The `llm2smt` and `eprover` examples, and eprover integration tests.
- Documentation moved to mdBook, with a scenario reference, an algorithm
  overview and a glossary.

### Changed

- `cache_db` defaults to `:memory:`, so a run no longer leaves a stray database
  behind.
- Solver status is stored in the cache.

### Fixed

- Improvement detection uses a strict `<`, so an equal-scoring challenger no
  longer replaces the incumbent endlessly.
- Failed and crashed runs are charged the penalty quality (`10_000_000`) and
  are **not** written to the persistent cache.
- The parameter parser accepts standalone conditions.

## [0.1.0] — 2026-04-10

First public release: a parallel Rust implementation of ParamILS with
BasicILS and FocusedILS, parallel evaluation over `(neighbour, instance)`
pairs, an SQLite result cache, a PyO3 extension exposing `specialize`, and the
ParamILS-compatible parameter-file syntax.

[Unreleased]: https://github.com/deeper4ai/ramparils/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/deeper4ai/ramparils/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/deeper4ai/ramparils/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/deeper4ai/ramparils/compare/v0.1.0...v0.1.2
[0.1.0]: https://github.com/deeper4ai/ramparils/releases/tag/v0.1.0
