---
name: tune-solver-wrapper
description: Write or review a RamParILS solver wrapper - the program that turns `-name value` parameters into a solver command line, runs it with limits, and prints the `#%# RamParIls #%#` result line. Covers PAR1, the UNKNOWN crash status, runhash, `--version`/`--params`/`--clean`, defaults for inactive conditional parameters, process cleanup and determinism. Use when wrapping a new solver for tuning or debugging a wrapper.
---

# RamParILS solver wrapper

The protocol is specified in `docs/reference/protocol.md`; the two real
wrappers to copy from are `examples/eprover/eprover_ho_wrapper.py` (E, via
solverpy) and `examples/primo/primo_wrapper.py` (an SMT solver, via solverpy).
`wrapper_template.py` beside this file is a solver-agnostic skeleton using
only the standard library. Pick one; do not mix styles.

## Required behaviour

**Invocation**: `<algo> <instance> <cutoff> -p1 v1 -p2 v2 ...` through
`sh -c`. Only *active* parameters are passed (inactive conditional children
are omitted), in alphabetical order.

**Result line** on stdout, exactly once:

```
#%# RamParIls #%# <status>, <runtime>, <quality>[, <runhash>]
```

Rules, each of which was a real bug once:

1. **PAR1**: any non-success — timeout, give-up, crash — reports
   `runtime = cutoff`, never the real elapsed time. A wrapper that reported a
   crash's true 0.01 s made crashing the best possible outcome, and the search
   climbed toward configurations that fail fast.
2. **`UNKNOWN` for wrapper-side failure**: solver missing, bad argument,
   exception before the solver ran, unparsable output. RamParILS logs
   `UNKNOWN` to the error log and keeps it **out of the cache**. An invented
   status like `ERROR` is cached as a legitimate result and logged nowhere —
   one run cached thousands of such "results" with an empty error log.
3. **Genuine solver failures keep their real status** (`ResourceOut`,
   `GaveUp`, SMT `unknown`): they are facts about the configuration and are
   cached.
4. **`runhash` only on success lines**: 16 lowercase hex digits, a hash of
   the solver's own deterministic search counters. Omit the field (not an
   empty value) otherwise — hashing an empty statistics block gives a constant
   that looks like "identical behaviour". Exclude counters that wobble between
   identical runs (for E: subsumption and term-bank counts). See
   `tune-solver-analysis` for what it is used for.

**`--version`**: print the wrapper's name and version, then the solver's own
`--version` output verbatim (or `<solver> MISSING`), then
`supports: version [runhash] [params] [clean]`. Exit non-zero if the solver
cannot be reached — RamParILS refuses to start, which is the point: a run
once spent 24 h against a binary that was not on `PATH`, every evaluation
quietly "timing out". If the solver's version string does not identify the
build (same version number for every commit), print a revision too.

**`--params -name value ...`** (optional, strongly recommended): print the
solver command line the parameters resolve to, run nothing. Use it to check
every domain value before a run, and from analysis scripts to turn exported
configurations into strategy files with *the same code* that ran them.

**`--clean -name value ...`** (optional): print the parameter list reduced to
what matters — inactive parameters and values equal to the solver default
dropped — for short, comparable strategy names.

## Implementation checklist

- **One `build_options(params)` function** that is the only place parameters
  become flags. `--params`, the solve path and offline strategy export all
  call it.
- **Every lookup tolerates a missing name** and falls back to the solver's own
  default, because inactive conditional children are not passed. Keep a
  `PARAM_DEFAULTS` table covering every parameter in the `.params` file, and
  reject names not in it (catches a domain/wrapper mismatch immediately).
- **Every parameter in the domain must reach the command line.** A parameter
  missing from the translation table is dropped without error and its whole
  neighbourhood scores identically. Test: for every parameter, for every
  value, `--params` output must differ from the default's output (unless the
  value *is* the default).
- **Expand encoded values** (indices into a heuristic table, short names for
  formulas) inside the wrapper; quote the expanded argument properly if a
  shell is involved, or better, call the solver with an argument list and no
  shell. An unquoted `(` in one strategy made `/bin/sh` fail every run.
- **Limits**: enforce the cutoff from outside the solver (process timeout,
  then kill the whole process group). A solver's own timeout that prints a
  normal-looking `unknown` and exits 0 is indistinguishable from a real
  `unknown`, which is cached and never recomputed at a longer cutoff — report
  a timeout as a timeout. Apply a memory limit (`RLIMIT_AS`, or `ulimit -Sv`)
  and make it overridable by an environment variable.
- **Reap children**: start the solver in its own session/process group and
  kill the group on timeout. Leaked solver processes from tuning runs
  survived for days and degraded later measurements on the same machine.
- **Determinism**: if the solver's behaviour depends on address-space
  randomisation (E's does), run it under `setarch $(uname -m) -R`, either in
  the wrapper or by launching RamParILS itself under it (inherited across
  fork/exec).
- **Fixed constants** that are not tuned (memory/deletion limits, always-on
  flags) live in the wrapper as named constants with a comment saying where
  the value came from.
- **Binary from `PATH`** by default, overridable by an environment variable;
  the `--version` block records which one ran.
- Print the solver's stdout/stderr (or a summary) to stderr on failure — it is
  what lands in the error log when a run needs debugging.
- Bump `WRAPPER_VERSION` whenever the translation changes; a wrapper is read
  at run time and nothing else attests which version a run used.

## Using solverpy inside the wrapper

If solverpy supports the solver, let it run and parse it — limits, status
vocabulary, statistics and runhash come for free, and evaluation later uses
the same parser:

```python
from solverpy.solver.atp.eprover import E

solver = E(f"T{round(cutoff)}-M{mem_giga}", binary=binary)
result = solver.solve(instance, strategy)       # strategy = build_options(...) as one string
valid = solver.valid(result)
status = result.get("status", "UNKNOWN") if valid else "UNKNOWN"
if status in solver.success:
    runtime, quality = result.get("runtime", cutoff), 0.0
    runhash = f", {result['runhash']:016x}"
else:
    runtime, quality, runhash = cutoff, 10_000_000.0, ""
print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}{runhash}")
```

solverpy limit strings take whole seconds; reject a fractional cutoff rather
than rounding silently.

## Testing a wrapper (no solver runs needed for most of it)

1. `./wrapper --version; echo $?` — with the solver present and with a bogus
   binary (expect `MISSING` and exit 1).
2. `./wrapper --params` with no arguments must reproduce the anchor/default
   strategy's command line exactly.
3. Loop over every parameter and value of the `.params` file through
   `--params`; every line must be accepted by the solver's option parser.
4. A handful of real solves on easy and hard instances, including one that
   times out, and one with an invalid value (expect `UNKNOWN`).
5. One RamParILS smoke run of a few minutes; read the log header and the
   error log.
