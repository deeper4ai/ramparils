---
name: tune-solver-eval
description: Evaluate solver strategies (tuned by RamParILS or not) on a test benchmark with solverpy, so that the numbers can be trusted - strategy files, the evaluation YAML, in-batch anchors and duplicate nulls, the noise floor, cached vs. measured results, train vs. test, subset-to-full transfer, and portfolio (greedy cover / union) scoring. Use when scoring tuned strategies or comparing any set of strategies.
---

# Evaluating strategies with solverpy

A tuning score is a property of the run (host, build, subset, cutoff, worker
count), not of the configuration. The only number that says whether a tuned
strategy is better is a **solverpy evaluation on a benchmark the tuner did
not see, at the deployment cutoff, in one batch with its anchors**.

## Setup

- **Strategy files**: `solverpy_db/strats/<sid>`, one line of solver options
  each, departures from the solver's defaults only (an empty file means the
  solver's own defaults). Generate tuned ones from the wrapper's own
  translation function (`tune-solver-analysis`, `conf_to_strat.py`). The
  `strats/` directory is the only record of what a sid means — keep it under
  version control.
- **Benchmarks** are bids: paths relative to `SOLVERPY_BENCHMARKS` (default:
  the current directory), a directory or a file listing problems.
- **Evaluation YAML**, run with `solverpy run eval.yaml`:

```yaml
evaluate: eprover            # solver plugin
common:
  limit: T10-M4              # whole seconds, memory in GB
  cores: 63                  # physical cores (minus one), never logical
  binary: eprover-ho         # if not the plugin default
  # force: true              # re-run even if cached -- HERE, never at top level
evals:
  benchmarks:
    - mybench/test
  strategies:                # or:  sidfile: eval.sids  (one sid per line)
    - anchor-default
    - anchor-best-known
    - dup-anchor-best-known  # byte-identical copy: the in-batch null
    - run03-init
    - run03-inc1
    - run03-final
    - run03-cover2
options:
  - outputs                  # keep solver outputs
  - premises                 # ATPs: record formulas used by each proof
```

Keys outside `common:`/`evals:` that look like evaluation settings are
**silently ignored**. `force: true` at the top level parses and does nothing;
confirm it took effect from the debug line `forced evaluation: db not
queried`, one per sid.

- **Re-measuring without overwriting**: append a dummy parameter to the sid
  (`anchor@v=2`) to get a fresh, separately cached measurement of the same
  strategy.
- If the solver is sensitive to address-space randomisation, launch with
  `setarch $(uname -m) -R solverpy run eval.yaml`.

## Making numbers trustworthy

1. **Anchors in every batch.** The default and the best known strategy, both
   re-measured in-batch. A cross-batch delta without an anchor is a claim,
   not a measurement.
2. **A duplicate null.** One strategy twice under two sids. Over time, ten or
   so nulls give the noise floor; in our clean batches the solved-count sd was
   ~0.1% of the benchmark, and **any gap under ~3 sd is nothing**.
3. **No oversubscription, no neighbours.** Workers ≤ physical cores; check
   the machine is otherwise idle and has no leftover solver processes. 1.75x
   oversubscription cost ~3% of solved instances and 2.7x the variance; leaked
   processes from an old tuning run silently cost another ~1%.
4. **One host, one build, one worker count per comparison.** Record the
   solver version separately — **a solverpy batch does not record which
   solver build ran**. Save `--version` output next to the batch.
5. **Measured vs. projected.** solverpy caches results across cutoffs and
   serves a shorter limit by *projecting* a longer cached run. That is correct
   caching, but a projection carries the load conditions of the batch it came
   from (the same strategy ran ~4% faster in a lighter batch, worth ~1% of
   solved instances near the cutoff). Never put projected baselines next to
   measured candidates in one table — use `force: true` or `@v=`.
6. **Never quote a running projection.** solverpy's in-progress `?N`
   extrapolation overshot a final result by ~4%: the tail is mostly timeouts.
7. **Write the prediction first** (in a note beside the YAML): which
   strategy wins, by roughly how much, what would surprise you.

## What to compute

- **Solved count and PAR-k** per strategy, deltas against the anchors, with
  gained/lost instance counts — a net of +5 can be +40/−35, and a failed
  idea can look like a success when only the net is read.
- **Train vs. test**: score on the tuning subset and on the rest. A gain that
  holds (or grows) on the unseen part is real; one that shrinks is fit to
  the subset.
- **Subset → full transfer**: when tuning used a subset, compare the subset's
  rank order with the full pool's at the *same cutoff* (simulate the shorter
  cutoff from cached runtimes rather than comparing different cutoffs).
  Naive scaling over-predicted the strongest strategies most.
- **Per-family breakdown**: gains often live in one family (in one campaign
  most of the tunable range was in one problem source). Report it; it is the
  main threat to validity.
- **Selection bias**: when picking the best of many candidates, split the
  benchmark in halves at random many times, pick on one half, score on the
  other; the median loss is the optimism of "best of N".
- **Cutoff sensitivity**: a gain at a short cutoff can shrink to almost
  nothing at a competition cutoff while the runtime speedup stays.
- **Portfolio value**: greedy cover over the solved sets, and the union of
  the best strategy with each candidate. Report each candidate's *added*
  instances. Strategies that lose on the mean but add many are the reason to
  keep them; a second solver or a guarded-sub-space variant repeatedly added
  more than the tuning did. For time-sliced schedules, split the cutoff
  between arms using the per-instance runtimes already measured (no extra
  runs needed).

## Report

A table per batch: sid, solved, delta vs. anchor, gained/lost, PAR-k, added
to the best strategy; the null pair's difference; host, cutoff, cores,
solver version, batch start/end times. Then the prediction, held or failed.
Feed the winners and the complementary strategies back into the next tuning
run (`tune-solver`) as starting points.
