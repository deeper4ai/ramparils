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

The perturbation draws uniformly from the *neighbourhood*, not from the parameters, so a
parameter with a large domain is perturbed more often than a boolean one: a five-valued parameter
offers four of the neighbours a boolean offers one.

<p align="center">
  <img src="../figures/basic-ils.svg" width="100%"
       alt="Basic ILS: initialization, first local search, and the main loop">
</p>

The figure is `approach: basic`, where every candidate is scored on the whole
instance set; FocusedILS wraps the same loop in a growing prefix, described below.

**Three configurations are in play at once, and keeping them apart is most of understanding the
search.** θ is the round's candidate. θ_base is the point each perturbation starts from — the ILS
*home base*. θ_inc is the incumbent: the best configuration seen, and what the run returns.

Note where each is written. The local search sets θ and, if the descent improved on it, θ_inc; the
acceptance criterion sets θ_base. **Only θ_base is perturbed**, so the incumbent improving does not
by itself move the search: once the home base stops moving, every later round samples the same ball
around a fixed point. That asymmetry is the reason for the knobs in the next section, and the source
of the failure they were added to fix.

---

## 🪂 Escaping a frozen home base

The acceptance criterion only ever replaces the home base with an at-least-as-good local optimum,
so on its own it cannot move the search uphill: once a strong local optimum is found, every later
round perturbs the same point and the run degenerates into repeated sampling from a fixed ball.
Four optional knobs address this. All are off by default, so a run that does not set them behaves
exactly as before they existed.

| Field | What it does |
|-------|--------------|
| `acceptance_tolerance` | Accept a *worse* local optimum as the home base while it stays within this relative margin of the **incumbent**. Measured against the incumbent and not against the home base on purpose: against the home base the margin compounds, and the home base can then drift downhill without limit. |
| `restart_failures` | Restart the home base after this many consecutive rejected local optima. Adapts to however many rounds the budget turns out to allow, which matters when a run gets tens of rounds rather than thousands. |
| `restart_probability` | ParamILS's `p_restart`: restart with this probability after each round. At the classic `0.01` it is calibrated for thousands of rounds — over 50 rounds it fires half a time. |
| `random_probes` | ParamILS's `R`: probe this many random configurations before the first descent, stepping to any that beats the starting configuration. Defaults to 0: RamParILS's primary use is specializing a strategy supplied by the caller, so the supplied configuration is the starting point unless asked otherwise. A run given no configuration at all starts from a single random draw, and these probes extend that. |

`restart_target` decides where a restart lands: `incumbent` perturbs the best configuration found
so far by `restart_strength` steps (default `2 × perturbation_strength`), while `random` draws a
uniformly random configuration, which is what ParamILS does. Restarts are logged so they can be
told apart from ordinary acceptance when a run is read back:

```
ils: restart: reason=stagnation target=incumbent strength=10 score=0.481578 instances=473 after 10 rejected local optima
```

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

**Random** (`approach: random`) is ParamILS's `pert_rand`: it uses all instances from the start,
but each round begins from a fresh uniformly random configuration and the acceptance criterion is
skipped entirely, so nothing carries over between rounds except the incumbent. That makes it a
random-restart baseline to measure an iterated local search against, not a tuning mode to prefer.
Restarts are inert under it, since every round already restarts.

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

**Pick the multiplier against the objective's ceiling, not in the abstract.** Under a PAR1 runtime
objective no instance can be charged more than `cutoff_time`, so a partial mean can never exceed
it and capping is *mathematically unable to fire* unless

```text
bound_multiplier × incumbent_score < cutoff_time
```

A multiplier just below that ratio is indistinguishable from `pruning: false`. With an incumbent
of 0.478 at a 1 s cutoff the ratio is 2.09, so the seemingly aggressive 2.0 prunes only a
candidate that has timed out on ~96% of the instances scored so far; the same 2.0 at a 10 s cutoff
with an incumbent of 2.95 caps at 59% of the ceiling and prunes normally. Express the intent as a
fraction of the ceiling and the multiplier follows.

Aggressive settings are safer than the "later results could lower the mean" caveat suggests,
because results arrive in completion order — the fast ones first. The running mean therefore
climbs as an evaluation proceeds, and once it has passed a bound above `1 × incumbent_score` it
rarely comes back down.

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
