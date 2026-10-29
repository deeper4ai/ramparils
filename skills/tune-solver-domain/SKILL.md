---
name: tune-solver-domain
description: Build, verify, compact and prune a RamParILS parameter space (.params file) for a solver - from the solver's help and source, or by reusing and extending an older domain. Covers conditionals vs forbidden clauses, reachability, dead and inert parameters, sentinel values, and compacting option families such as E's clause-selection heuristics into fixed blocks. Use when writing or revising a parameter domain for tuning.
---

# Parameter domain for RamParILS

The syntax is in `docs/reference/params.md` (read its "Designing a space"
section too). This skill is about getting the *content* right: a domain
decides what a run can find, and most failed tuning runs in our record failed
here, silently.

## What a domain costs

- Every parameter contributes `|domain| - 1` neighbours to **every** local
  search step. A 5-valued parameter is four times as expensive as a boolean,
  and is perturbed four times as often — which looks like importance and is
  only sampling.
- Count before running: number of configurations, neighbourhood size at the
  starting configuration, and which parameters are **inactive at the start**
  (behind a guard that is off). A short script over the `.params` file is
  enough; do it every time the domain changes.
- `perturbation_strength` should scale with the parameter count:
  `max(2, ceil(0.2 * n_params))` (ParamILS's rule).

## Building a new domain

1. **Collect the options from the solver itself**, not from memory: `--help`,
   the man page, and the source where options are declared and handled. For E:
   `PROVER/e_options.h` (every option's argument type and default),
   `PROVER/eprover.c` (the handlers — some accept values `--help` does not
   list), and the files that hold name tables (e.g. the grounding strategy
   names for `--satcheck`). For an SMT solver: its options struct and where
   each field is read.
2. **Select options that change search behaviour**: inference rules,
   selection/ordering heuristics, preprocessing, simplification levels.
   Leave out resource and incompleteness knobs (memory limits, deletion
   limits, output options) — fix those in the wrapper as constants, choosing
   the value the anchor strategy uses, and say so in a comment.
3. **Values**: enumerate the values that mean different things. For numeric
   options use a coarse ladder (e.g. powers of 2 around the default), but
   check for cliffs: one option scored flat on a plateau for most values and
   dropped ~5% at one of them — resolution matters where the response is
   not smooth.
4. **Defaults**: the solver's own default, or the anchor strategy's value.
   Prefer **starting from the best known strategy** (`initial_config_file`)
   over starting from solver defaults; verify the domain can express that
   strategy *exactly*, and write down which parameter values reproduce it.
5. **Verify every value against the real binary** before the first run.
   Inherited domains go stale: two values copied from an older domain
   (a selection function that had been renamed, and an ordering value valid
   for a different flag one line down) made thousands of evaluations crash,
   which also made every neighbour move toward them permanently unreachable.
   Many solvers print the accepted list when given a bad value
   (`eprover --literal-selection-strategy=none` lists valid names); diff that
   list against the domain. Then run the wrapper's `--params` for every value
   of every parameter (no solve) and check the command lines.

## Reusing an older domain

- **Extend as a superset**: keep every old parameter's name, domain and
  default, so configurations from the old space still mean the same thing and
  old strategies can seed the new run.
- Re-verify the inherited values against the *installed* build (see 5 above).
- Write the domain's history in its header comment: what was added, what was
  removed and why, with the evidence (a run, a source line). Removed parts
  should be recoverable from version control, not deleted from memory.
- A changed space needs a **fresh cache**: the cache key is the active
  configuration, so old rows may be wrongly reused or never hit.

## Sentinels and encodings

- `none` (or `off`) as "omit the flag, solver default" is fine, but it must
  have **one spelling**. If the solver also accepts an explicit name for the
  default (E's `NoGrounding`), leave that name out of the domain, or two values
  will mean the same thing and double the cost.
- Distinguish "omit the flag" from a real value that looks like "off": E's
  `--definitional-cnf=0` actively disables definitional CNF, which is not the
  same as omitting the flag (default 24). Mixing them changes what is measured
  without any error.
- Values must be **shell-safe**: RamParILS runs the wrapper through `sh -c`.
  Encode anything with parentheses, commas or spaces (heuristic formulas,
  weight functions) as an index or short name that the wrapper expands.

## Conditionals, not forbidden clauses

- `child {..} [d] | parent in {..}` whenever `parent` decides whether `child`
  is read. This is not documentation: inactive children are left out of the
  cache key, so all their settings collapse into one evaluation. Without the
  condition the search evaluates all of them, gets identical scores, and you
  see a plateau.
- **Find hidden gates in the source.** An option that skips constructing a
  component silently disables every option that component reads. In one SMT
  solver a single branching flag skipped building the arithmetic propagator
  (no else branch), so nine other options were parsed, passed and never read;
  that flag measured −5.5% of solved instances and nobody would have guessed
  from `--help`.
- Use a forbidden clause `{a=x, b=y}` only for combinations that are invalid
  or duplicate another configuration.

## Reachability: the conditional-parameter trap

A guard that is off in the starting configuration is flipped *alone*, with
its dependents at whatever values they hold. If it does not pay at those
values, first-improvement descent rejects it and the sub-space is never
entered — in one campaign an option one flip from the default was never
switched on in ~930 descents, and the most complementary strategy of the
campaign lived behind it. Mitigations:

- prefer spaces where everything is active at the starting configuration;
- enumerate a guarded sub-space offline (a small grid evaluated with
  solverpy) to learn whether it is bad or only badly initialised;
- use `perturbation_strength` ≥ 3, so a perturbation can open a guard and move
  its dependents in the same walk;
- note that `restart_target: random` re-rolls inactive dependents too.

## Does the parameter do anything?

Before a parameter enters the space, run the solver at two extreme values on
a few instances and confirm **the solver's own counters** (processed clauses,
decisions, conflicts, pivots) change. Structural checks cannot catch an
option that parses and is ignored. Seen in practice:

- options written by the CLI and read by no code (dead until a later commit);
- a parameter missing from the wrapper's translation table — dropped silently;
- a random seed that is inert unless a separate randomisation switch is on;
- E's `--sos-uses-input-types`, inert unless a heuristic uses the
  `SimulateSOS` priority — so inert for every heuristic in our table.

After a run, the cache tells you more (`tune-solver-analysis`: runhash
collision groups, moves accepted alone below the noise floor).

## Pruning and compaction after runs

Feed what a run showed back into the domain:

- **Remove** parameters proven inert (globally, or for this benchmark family:
  options only read by a code path this logic never reaches). Keep a note.
- **Conditionally inert** is not dead: E's `--satcheck` grounding choice only
  matters after `--satcheck-proc-interval` processed clauses, so at a 1–2 s
  cutoff many values are indistinguishable; at a longer cutoff they are not.
  Decide by the deployment cutoff, not the tuning cutoff.
- **Shrink** domains to the values that were ever accepted or that the
  evaluation says differ.
- **Compact families of coupled parameters into fixed blocks.** Independent
  parameters for things that are only ever used together multiply the
  neighbourhood for nothing.

### Example: E's clause-selection heuristic

E's heuristic is a weighted list of clause evaluation functions (CEFs):
`--define-heuristic='(f1*CEF1, f2*CEF2, ...)'`. Two encodings:

**Independent pairs** — a slot count plus a CEF and a frequency per slot:

```
slots {0,1,2,3,4,5} [0]
heur1 {nb7,nb7dd,fifo,nb7ng,refgoals} [nb7] | slots in {1,2,3,4,5}
freq1 {1,2,6}                         [2]   | slots in {1,2,3,4,5}
heur2 ...                                   | slots in {2,3,4,5}
...
```

5 CEFs × 3 frequencies = 15 choices per slot, as two parameters. In practice
a long descent never changed *which* CEF a slot used — only its frequency —
while the neighbourhood paid for every combination.

**Fixed blocks** — one index per slot into a table of (frequency, CEF) pairs,
expanded by the wrapper:

```
# 0:1*nb7 1:2*nb7 2:1*nb7dd 3:2*nb7dd 4:6*nb7dd 5:1*fifo 6:2*fifo
# 7:1*nb7ng 8:2*nb7ng 9:1*refgoals 10:2*refgoals
slots {0,1,2,3,4,5} [0]
heur1 {0,1,2,3,4,5,6,7,8,9,10} [1]  | slots in {1,2,3,4,5}
heur2 {0,1,2,3,4,5,6,7,8,9,10} [4]  | slots in {2,3,4,5}
heur3 {0,1,2,3,4,5,6,7,8,9,10} [5]  | slots in {3,4,5}
heur4 {0,1,2,3,4,5,6,7,8,9,10} [7]  | slots in {4,5}
heur5 {0,1,2,3,4,5,6,7,8,9,10} [10] | slots in {5}
```

The table holds only pairs that were used by the anchor strategy or by
accepted moves (plus one exception so the anchor stays expressible exactly).
The defaults reproduce the anchor at `slots: 5`. `examples/eprover/` has both
styles: `params-eprover.txt` / `eprover_wrapper.py` (independent pairs) and
`params-eprover-ho.txt` / `eprover_ho_wrapper.py` (fixed table).

The same pattern applies to any solver option that is a list of
(weight, component) items, a portfolio schedule, or a family of thresholds
that only make sense together.

### When to stop searching and enumerate

When the space is down to a handful of parameters that matter (say ≤ 5
booleans), measure the whole cube with solverpy on the test benchmark. Main
effects do not add — in one 32-cell cube three individually small options
nearly cancelled each other — and an enumerated space is closed: nothing a
search can find there is not already in the table.
