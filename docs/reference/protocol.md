# 🔌 Solver wrapper protocol

RamParILS never talks to the target algorithm directly — every evaluation goes through a
**wrapper**: a small executable that translates a `-name value` parameter list into the
algorithm's real command line, runs it, and reports back over stdout in a fixed text format.
The wrapper is the only thing that needs to know how to invoke the algorithm; RamParILS itself
only ever speaks this protocol.

Two real wrappers ship as worked examples and are the reference to copy from:
[`examples/primo/primo_wrapper.py`](https://github.com/deeper4ai/ramparils/blob/main/examples/primo/primo_wrapper.py)
(the primo QF_LRA solver, via `solverpy.solver.smt.primo`) and
[`examples/eprover/eprover_wrapper.py`](https://github.com/deeper4ai/ramparils/blob/main/examples/eprover/eprover_wrapper.py)
(the E prover, via `solverpy.solver.atp.eprover`). Both are Python and use
[SolverPy](https://github.com/ai4reason/solverpy) to run and parse the underlying solver, but
nothing about the protocol requires either — a wrapper is any program the shell can run.

## 📥 Invocation

For each evaluation, RamParILS runs:

```
<algo> <instance> <cutoff_time> -param1 val1 -param2 val2 …
```

- `<algo>` — the command from the scenario's `algo` field, e.g. `"python3 primo_wrapper.py"`
- `<instance>` — path to the instance file
- `<cutoff_time>` — per-run time limit in seconds
- `-param val` pairs — the **active** parameters (per the parameter file's conditionals), in
  alphabetical order by name

The complete command is passed to `sh -c`. Scenario files and parameter values must therefore be
trusted, and wrappers should avoid paths or values that need shell quoting.

Example, resolved from `algo: "python3 primo_wrapper.py"`:

```
python3 primo_wrapper.py /data/QF_LRA/inst1.smt2 30.0 -lra_model_phase true -theory_phase polarity
```

## 📤 Result line

The wrapper must print one result line to **stdout**:

```
#%# RamParIls #%# <status>, <runtime>, <quality>[, <runhash>]
```

| Field | Values | Description |
|---|---|---|
| `status` | text | Outcome of the run, stored verbatim in the cache and in `ramparils db status` exports |
| `runtime` | float, seconds | Charged runtime — see PAR1 below |
| `quality` | float | Numeric cost to minimise when `run_obj: quality`; conventionally `0.0` on success |
| `runhash` | 16 hex digits, *optional* | Fingerprint of the solver's internal work on this instance — see [below](#-the-optional-runhash-field) |

The line may appear anywhere in stdout; everything else on stdout and stderr is ignored (but
still worth printing — it is what lands in the debug/error logs when a run needs debugging).
RamParILS stores `status` for reporting but does not interpret it when scoring; it is the
wrapper's own contract with itself. **If no valid result line is found at all**, RamParILS
synthesizes one: status `UNKNOWN`, runtime `cutoff_time`, quality `10000000`. `UNKNOWN` results
are excluded from the persistent cache and logged to the run's error log — the wrapper can, and
for real crashes should, emit this itself; see below.

### Examples, by solver status vocabulary

A wrapper's `status` values are whatever the underlying solver reports — RamParILS doesn't
constrain the vocabulary, only how the two special outcomes (success, `UNKNOWN`) are used. Two
worked families, matching the two example wrappers:

**SMT (`primo_wrapper.py`, via SolverPy's `smt` status plugin)** — success is `sat` / `unsat`;
`unknown` is a real, cacheable non-success answer distinct from a crash:

```
#%# RamParIls #%# sat, 1.234500, 0.0, 3f9a1c7b2e6d4085
#%# RamParIls #%# unsat, 0.087200, 0.0, 9c1e0a2f7b6d5443
#%# RamParIls #%# unknown, 30.000000, 10000000.0
#%# RamParIls #%# UNKNOWN, 30.000000, 10000000.0
```

**TPTP/SZS (`eprover_wrapper.py`, via SolverPy's `tptp` status plugin)** — success is `Theorem` /
`Unsatisfiable` / `Satisfiable` / `CounterSatisfiable` / `ContradictoryAxioms`; `ResourceOut`,
`Timeout` and `GaveUp` are real non-success answers:

```
#%# RamParIls #%# Theorem, 4.812000, 0.0, 812babf67d10cf3d
#%# RamParIls #%# Unsatisfiable, 0.930000, 0.0, e74b8847f11fb0e8
#%# RamParIls #%# ResourceOut, 30.000000, 10000000.0
#%# RamParIls #%# GaveUp, 30.000000, 10000000.0
#%# RamParIls #%# UNKNOWN, 30.000000, 10000000.0
```

Note what both families share: **only a success line carries a `runhash`**, and **every
non-success line — real or `UNKNOWN` — charges the full `cutoff_time`, never the solver's actual
elapsed time**. Both are deliberate, not incidental; see the two subsections below.

## ⏱️ PAR1 — a failure must never look cheap

`runtime` on a non-success line must be `cutoff_time`, not whatever time the process actually
took — including a crash that failed in milliseconds. This is the standard **PAR1** (penalized
average runtime, ×1) convention: a run scored on `run_obj: runtime` treats a lower number as
better, so a wrapper that reports a crash's true near-instant runtime makes crashing look like
the *best* possible outcome, and the search climbs toward configurations that reliably fail fast
instead of ones that actually solve instances.

```python
if status in solver.success:
    runtime = result.get("runtime", cutoff)   # real elapsed time
else:
    runtime = cutoff                          # PAR1: always the full budget, however it failed
```

This was a real bug, not a hypothetical: an early version of `eprover_wrapper.py` reported real
elapsed time unconditionally, and a batch of invalid parameter values (below) made ~43% of
evaluations fail near-instantly — all scoring better than genuine solves.

## 🆘 `UNKNOWN` — a genuine crash is not a new status

When the wrapper itself cannot produce a real result — the solver binary is missing, an
argument was rejected, an exception was raised before the solver could even start — report it as
status `UNKNOWN`, reusing RamParILS's own sentinel rather than inventing something like `"error"`
or `"CRASH"`. `UNKNOWN` is the **only** status RamParILS treats specially:

- it is written to the run's error log (`log_crash`), which is otherwise the one place a human
  would notice something went wrong;
- it is **excluded from the persistent cache** — a wrapper-side crash is not a fact about the
  configuration and must not be remembered as one.

An invented status (say, `"ERROR"`) gets neither: RamParILS doesn't recognise it, so it is cached
as an ordinary, cacheable, permanent-looking result, and nothing is logged anywhere. This is
exactly the failure a real run hit: two stale parameter values made the solver exit non-zero on
a large fraction of evaluations, and because the wrapper reported them under a made-up status,
every one of those "crashes" was silently cached as a legitimate outcome and the error log —
the only place that would have shown it — stayed empty for the whole run.

```python
try:
    result = solver.solve(instance, strategy)
except (KeyError, OSError, ValueError) as error:
    print(f"wrapper error: {error}", file=sys.stderr)
    status, runtime, quality, runhash = "UNKNOWN", cutoff, FAILURE_QUALITY, ""
else:
    status = result.get("status", "UNKNOWN") if solver.valid(result) else "UNKNOWN"
    # ... success/PAR1 branch as above ...
```

A genuine solver-reported failure (`ResourceOut`, `GaveUp`, SMT's `unknown`, …) is *not*
`UNKNOWN` — it keeps its real status and stays cached, because it is a fact about that
configuration on that instance, reproducible on a re-run. `UNKNOWN` means specifically "the
wrapper could not get an answer," not "the answer was negative."

## #️⃣ The optional `runhash` field

A fourth, optional field carries a **runhash**: an 8-byte (16 hex digit) fingerprint of the
solver's own internal work on this instance — a hash over a selected subset of the solver's own
result counters (`RunHash` in SolverPy), independent of wall-clock runtime. Two runs of the same
solver build on the same instance that did byte-identical internal work produce the same
runhash; a parameter that changed nothing observable produces the same runhash as the run
without it, which is exactly the signal needed to catch a **structurally dead parameter** — one
that parses, reaches the command line, and changes nothing, which otherwise shows up only as an
unexplained plateau in the search (see [Designing a space](params.md#-designing-a-space)).

Rules for emitting it:

- **only on a success line.** A crashed or capped run has no solver counters to hash — hashing
  an empty/default selection would produce a fixed constant that reads as "identical behaviour"
  between runs that share nothing but having produced no data;
- 16 lowercase hex digits, no `0x` prefix;
- omit the field entirely (not an empty value) when not applicable — the trailing comma and
  value are absent, not blank.

`ramparils` stores it per result (nullable, so old caches without it still open) and XORs it
across a descent's evaluated neighbours for a per-round fingerprint; `ramparils db status` and
`ramparils db runhashes` export it for offline analysis (`work26/expericon/scripts/group-runhashes.py` groups
strategies that produced the same runhash).

## 🆚 `--version`

Before the first evaluation, RamParILS runs `<algo> --version` and refuses to start unless it
succeeds — this is the fix for a run that silently spent 24 hours tuning against a solver binary
that was never on `PATH`, reporting nothing wrong because every evaluation quietly "timed out"
instead of failing to launch. The wrapper must:

- **exit `0`** when it could reach the inner solver, **non-zero** otherwise;
- print its own version, then the inner solver's own `--version` output verbatim — or, if the
  solver could not be reached, a `<solver> MISSING` placeholder line in its place, keeping the
  block's shape the same either way so the trailing line is always `supports:`;
- **end with** a line `supports: <feature> <feature> …` — space-separated keywords naming what
  this wrapper implements. `version` itself must always be listed; RamParILS checks for it and
  refuses to start if it's absent, so a wrapper that answers `--version` at all but omits the
  keyword is treated the same as one that doesn't answer it.

| `supports` keyword | Means the wrapper also |
|---|---|
| `version` | answers `--version` per this section (required) |
| `runhash` | emits the optional fourth field on success lines |
| `params` | answers `--params [-name value ...]` — prints the resolved solver command line for a given parameter set, and exits without running anything; useful for inspecting what a configuration actually resolves to |

RamParILS logs the whole `--version` block once, at startup, separated from the per-instance
solver stats. This is what lets a run's own log attribute its results to an exact solver build
— `primo --version` prints `primo 0.1.0` for every build ever made, so without this a build
mismatch across two runs is invisible until someone compares scores and can't explain the gap.

Reference implementation (`primo_wrapper.py`):

```
$ primo_wrapper.py --version
primo_wrapper.py 0.3.0
primo 0.1.0 (git b3c4188)
supports: version runhash params
```

With the solver unreachable — the wrapper still prints the full block, but exits non-zero, so
RamParILS refuses to start rather than running for hours against a solver that was never there:

```
$ primo_wrapper.py --version; echo "exit: $?"
primo_wrapper.py 0.3.0
primo MISSING
supports: version runhash params
exit: 1
```

## 🧩 Full example — the crash/success branch

The essential shape, distilled from `eprover_wrapper.py`'s `main()`:

```python
try:
    result = solver.solve(instance, strategy)
except (KeyError, OSError, ValueError) as error:
    print(f"wrapper error: {error}", file=sys.stderr)
    status, runtime, quality, runhash = "UNKNOWN", cutoff, FAILURE_QUALITY, ""
else:
    valid = solver.valid(result)
    status = result.get("status", "UNKNOWN") if valid else "UNKNOWN"
    if status in solver.success:
        runtime = result.get("runtime", cutoff)
        quality = 0.0
        runhash = f", {result['runhash']:016x}"
    else:
        runtime = cutoff          # PAR1
        quality = FAILURE_QUALITY
        runhash = ""

print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}{runhash}")
```

Three rules, all visible in this shape and all covered above: a crash is `UNKNOWN`, not an
invented status; every non-success line — crash or genuine failure alike — charges the full
cutoff; and `runhash` is present if and only if `status` is a success.

## 💬 Design notes

- **`--params` and dry-run inspection.** Because `--params` resolves a parameter set to a
  command line without running anything, it's the fastest way to sanity-check a parameter file
  against the actual wrapper: `primo_wrapper.py --params -lra_model_phase true` prints exactly
  what the solver will see, catching a typo'd flag name before it costs a single evaluation.
- **Prefer reusing `UNKNOWN` over adding a wrapper-specific error status.** It is not a stylistic
  preference — an invented status silently opts a whole failure class out of both the error log
  and cache exclusion, and the failure is invisible until a human notices the numbers don't add
  up, which is what actually happened. Reach for a new status only when it's a genuine outcome
  the solver itself reports.
- **Verify a parameter's domain against the real binary, not an inherited one.** The 43%-error
  incident above was two stale values (valid in an older, different codebase's domain, rejected
  by the version actually installed) — `eprover -W none` / `-G none` printed the accepted-values
  list as part of a "wrong argument" error, and diffing that against the parameter file's domain
  found both. Cheap to check before trusting a domain copied from elsewhere.
