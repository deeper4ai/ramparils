# 🧪 E prover tuning example

This tunes the `eprover` ATP solver via `solverpy`'s `E` class, through
`eprover_wrapper.py` -- no `grackle` involved. An older example here was
built on `solverpy_grackle.trainer.eprover`/`solverpy_grackle.runner.
eprover.EproverRunner` and `ramparils.specialize()` (`run.py`, `run-nb5.py`,
`strategies.py`, `grackle-eprover.sh`, `params-eprover*.txt`, `scenario.yaml`,
`run.sh`); it has been removed in favour of this one and is recoverable from
git history if ever needed. `bushy010/` (the benchmark instances) is kept --
this example still uses it.

Run the example from this directory:

```bash
cd examples/eprover
ramparils run scenario-eprover.yaml
```

The working directory matters because the scenario refers to the wrapper,
parameter file, instance list, cache and logs using relative paths.

The wrapper runs `eprover` from `PATH`, via `solverpy`'s `E` class (its
default `binary`). To use a different build, put it on `PATH` under that name
(a symlink is enough); the wrapper does not read an environment variable for
this, matching `examples/primo/primo_wrapper.py`'s convention.

Each solver run has a 4096 MiB virtual-memory limit by default, applied via
`ulimit -Sv` around the eprover invocation (independent of E's own
`--memory-limit=2048` baked into `solverpy.solver.atp.eprover.E_STATIC`).
Override it with `EPROVER_MEMORY_MB`, for example:

```bash
EPROVER_MEMORY_MB=8192 ramparils run scenario-eprover.yaml
```

## 🎛️ Parameter space

`params-eprover.txt` is a deliberately **small** domain, not the full
one `solverpy_grackle.trainer.eprover.default.DefaultDomain` builds (core + HO
+ ordering + a per-slot weighted multi-heuristic search). It keeps:

- **Core proof-search switches** (from `solverpy_grackle.trainer.eprover.
  core.CoreDomain`, a subset): `sel` (literal selection), `simparamod`
  (paramodulation), `der` (destructive equality resolution), `fwdemod`
  (forward demodulation level), `defcnf` (definitional CNF), `condense`,
  `presat`, `prefer`.
- **Term ordering** (from `...eprover.ordering.OrderingDomain`, in full):
  `tord`, `tord_prec`, and the two parameters conditional on `tord in {KBO6}`:
  `tord_weight`, `tord_const`.
- **Up to 4 clause-selection heuristic slots**, the same shape as
  `...eprover.heuristic.HeuristicDomain` builds, but drawing from 5 fixed
  named CEFs per slot instead of that module's full set of 20. `slots {0,1,
  2,3,4}` picks how many are active; `0` emits no `-H`/`--define-heuristic`
  flag at all (E's own built-in heuristic). Slot *N* (1-4) contributes
  `heurN` -- one of `nb7`, `fifo`, `precasc`, `mzr02`, `bls`, the CEF half of
  individual `(freq, CEF)` pairs drawn from that module's `HEURISTIC_CEFS`
  (indices 0, 2, 5, 15, 19 -- chosen to span distinct families) -- and
  `freqN {1,2,3,4,8,16}`, its weight, tuned **independently** of which CEF it
  multiplies rather than moving together as one fixed pair the way
  `HEURISTIC_CEFS` itself bakes them. `heurN`/`freqN` are conditional on
  `slots in {N, N+1, ..., 4}` (e.g. `heur2`/`freq2` need `slots >= 2`), so
  RamParILS supplies exactly the first `slots` many pairs and the wrapper
  concatenates them in order. Whatever slots are active are terminated with
  the same `1*FIFOWeight(ConstPrio)` fallback for completeness, matching
  `EproverRunner.args`'s convention.

No HO-specific options and no SinE/relevancy pruning are in this space at
all -- `solverpy_grackle.trainer.eprover.ho.HoDomain` and `.sine.SineDomain`
are not included.

**Watch the sentinel values**, because they are not all the same shape:

- `simparamod`, `der`: `"none"` means no flag -- E's own default.
- `fwdemod`: `"2"` means no flag -- **also** E's own default, but note it is
  the numeric value the domain otherwise treats as ordinary, not a distinct
  string like `"none"`.
- `defcnf`: `"none"` means no flag -- E's own default. **`"0"` is a real,
  distinct value**, not a second way to say "off": per `eprover --help`,
  `--definitional-cnf=0` actively disables definitional CNF, which is not the
  same as omitting the flag and inheriting E's default (currently 24). Mixing
  these up silently changes what a run measures rather than erroring.
- `sel`, `tord`, `tord_prec`: **no "no flag" sentinel at all** -- always
  emitted as `--flag=value`, whatever value RamParILS supplies, even when it
  happens to equal E's own default. This matches
  `solverpy_grackle.trainer.eprover.core.CoreDomain`'s own comments (`# runner
  DEFAULT`): these three never had an omit-the-flag path in the original
  domain either.
- `tord_weight`, `tord_const`: conditional on `tord in {KBO6}`. RamParILS
  omits them from the command line while `tord` is `LPO4`, so the wrapper
  looks every name up defensively and falls back to E's own default when one
  is absent -- same convention as `examples/primo/primo_wrapper.py`.

## 📤 Output

The wrapper answers two standalone flags, neither of which runs a solve
(IDEAS.md item 1 and item 3, ported from the primo wrapper):

- `eprover_wrapper.py --version` prints its own name and version, `eprover
  --version` verbatim, and a trailing `supports: version runhash params`
  line.
- `eprover_wrapper.py --params [-name value ...]` prints the E command-line
  options the parameter list resolves to, without an instance or a solve.

The wrapper reports `Satisfiable`, `Unsatisfiable`, `Theorem`,
`CounterSatisfiable` and `ContradictoryAxioms` as successful (`solverpy`'s
`Tptp` status plugin, `complete=True`). `GaveUp`, process errors, and
timeouts (`ResourceOut`/`Timeout`) receive the full cutoff runtime and a
large quality penalty.

On a successful run the wrapper appends a fourth field to the result line,
`status, runtime, quality, runhash`: a fingerprint (`solverpy`'s
`RunHash`/`E_RUNHASH_GEN`) of E's internal search counters, independent of
runtime. It is omitted for anything but a successful run, for the same
reason `examples/primo/primo_wrapper.py`'s README gives: a run that never
produced statistics would otherwise hash a fixed, empty-selection constant
that carries no information. **`E_RUNHASH_GEN` has its own noise floor**,
found by rerunning the same configuration three times: `Subsumes` and
`TermBank` wobble by a handful of counts even on a fully-reproduced, byte-
identical proof (E can settle ties -- term/literal orderings, or scheduling
among equal-cost choices -- with genuine internal nondeterminism), so they
are excluded from the runhash while staying in the general result table. See
`solverpy/packages/solverpy/src/solverpy/solver/atp/eprover.py` for the full
list and which half of each later-added group is active versus commented out
for later.
