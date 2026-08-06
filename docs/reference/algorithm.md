# 🧠 Algorithm

RamParILS implements **Iterated Local Search (ILS)** for automated algorithm configuration.
The goal is to find the parameter setting of a target algorithm that minimises runtime or a
numeric quality cost on a set of training instances. Both objective modes are minimisation;
wrappers for maximisation problems must transform utility into a cost.

---

## 🔄 Iterated Local Search

ILS alternates between two phases: **local search** finds a local optimum in the configuration
space, and **perturbation** escapes it by applying a random walk.  Starting from an initial
configuration (typically the parameter file's declared defaults), local search explores the
neighbourhood one parameter at a time, greedily accepting any neighbor that improves performance.
When no improving neighbor exists, perturbation applies `perturbation_strength` random steps to
escape the local optimum, and the cycle repeats until the tuner timeout expires.

The neighbourhood of a configuration is all configurations that differ in exactly one parameter.
For a space with P parameters and average domain size D, each configuration has at most (D−1)×P
neighbors. RamParILS submits neighbours to a bounded worker pool and accepts the first fully
evaluated improvement. The wall-clock cost still depends on neighbourhood width, fidelity,
worker count, cache hits, and solver runtimes.

---

## ⚖️ BasicILS vs FocusedILS

The `approach` field selects the ILS variant.

**BasicILS** (`approach: basic`) evaluates candidates on the complete training-instance set
before comparing their aggregate scores.

**FocusedILS** (`approach: focused`, the default) uses progressive global fidelity. Candidates
are compared by their aggregate score on the current prefix of the training-instance list. When
the incumbent survives a challenge, the prefix grows by `fidelity_step` until all instances are
used. A challenger must have a strictly lower score at the current fidelity to replace the
current configuration.

RamParILS starts FocusedILS at `initial_fidelity` instances per configuration and increases that
global fidelity by `fidelity_step`, up to the number of available instances. With W workers and
fidelity F, the current scheduler can approximately evaluate `ceil(W/F)` different neighbors at
once. Increasing F therefore uses more workers on instances of the same neighbor and reduces
speculative work on neighbors that may become irrelevant after the first improving move.

Fidelity always uses the first F entries in the supplied instance list. The list is not shuffled,
so its early prefixes should be reasonably representative of the complete training set.

A score is only meaningful relative to the prefix it was measured on — different fidelities are
different objective functions, and comparing across them is meaningless. Two configurations
outlive a fidelity increase: the incumbent, and the local optimum the next perturbation starts
from (the ILS home base). **Both are re-measured on the new prefix whenever the fidelity grows**,
so every comparison the ILS makes is between scores taken on the same instances. Each increase is
logged as

```text
ils: n_runs increased to 64/1753 incumbent_score=0.456619 home_base_score=0.456619
```

The home base is usually the incumbent, in which case the second measurement costs nothing.

Home-base replacements are logged too, one line each, with the parameter diff against the
previous home base rather than a full configuration block — it can change every round:

```text
ils: new home base: hash=bd4315273a9356dd score=0.025000 instances=4 changes: alpha: 3 -> 2; beta: a -> b
```

Replacements with no effective change (a differing value on a parameter whose guard is off)
produce an empty diff and are not logged.

This matters more than it may appear. Prefix means drift as the prefix grows — typically upward,
if the early instances are cheaper — while the acceptance criterion is monotone: the home base is
only ever replaced by something that beats it. A home base left on an old, smaller prefix
therefore holds an optimistically low bar that the only mechanism able to update it can no longer
clear, and the perturbation centre freezes for the rest of the run. ParamILS avoids this by
storing a score per fidelity level for every configuration and always comparing two states at
their common level (`isBetterWithLesserDetail` in `param_ils_2_3_run.rb`); RamParILS keeps one
score per state and re-measures instead.

**Random** (`approach: random`) currently uses all instances from the start and otherwise follows
the same ILS path as `basic`. It should not currently be interpreted as an independent uniform
random-search baseline.

---

## ✂️ Adaptive capping

Adaptive capping (controlled by `pruning` and `bound_multiplier`) stops evaluating a configuration
early if its partial mean exceeds `bound_multiplier × incumbent_score`. This is a heuristic:
later results could lower the final mean, so capping can change which configurations are explored
and accepted.

A lower `bound_multiplier` (e.g., 2.0) prunes more aggressively and speeds up search, but may
occasionally discard a configuration that would have been competitive on later instances.
The default of 10.0 is conservative for positive cost objectives. Set `pruning: false` when exact
uncapped comparisons are required or when partial means and multiplier bounds are inappropriate
for the objective.

---

<a id="iterative-deepening"></a>

## 📈 Iterative deepening

Iterative deepening (`iterative_deepening: true`) runs ILS in multiple phases with an exponential
schedule rather than a single run.  Early phases use a small fraction of the training instances
and a short per-run cutoff — just enough to quickly filter the search space and find a good
starting point.  Later phases gradually increase the instance count, cutoff time, and per-phase
budget, refining the best region found so far.  The incumbent from each phase seeds the next,
so early exploration and late refinement share information.

Three growth factors control the schedule:

| Field | Controls | Effect of larger value |
|-------|----------|------------------------|
| `lambda_n` | geometric instance-count growth | larger values use more instances in early phases |
| `lambda_c` | geometric cutoff growth | larger values use longer cutoffs in early phases |
| `lambda_t` | geometric cumulative-deadline growth | larger values give earlier phases later deadlines |

All three default to `0.5`, giving an approximate geometric doubling schedule.
The timeout values are cumulative deadlines measured from the start of
iterative deepening, not independent budgets added together. Each phase gets
the time remaining before its deadline.

Iterative deepening is most useful when the training set is large (hundreds of
instances) and the cutoff time is long (tens of seconds), making a full-budget
single run prohibitively slow at the start.
