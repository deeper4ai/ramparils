# 📖 Glossary

---

**Active parameter**
A parameter that is included in the solver invocation for a given configuration.
Conditional parameters are only active when their parent parameter has the required value;
inactive parameters are omitted from the command line entirely.

---

**Adaptive capping** (see also: *pruning*)
A heuristic early-stopping rule that abandons evaluation when a candidate's partial mean
exceeds `bound_multiplier × incumbent_score`. Later results could lower the final mean, so
capping can change the search trajectory. Controlled by the `pruning` and `bound_multiplier`
scenario fields.

---

**Configuration**
A complete assignment of values to all parameters in the parameter space.
Also called a *strategy* in the context of solver portfolios.
Represented internally as `{name → value}` string maps.

---

**Conditional parameter**
A parameter whose domain is only meaningful when a *parent* parameter has a specific value.
Declared in the `.params` file as `child {…} [default] | parent in {val}`.
Conditional parameters that are inactive are omitted from the solver command line.

---

**Cutoff time** (`cutoff_time`)
The per-run time limit in seconds passed to the target algorithm.
The solver wrapper is expected to respect this limit and report `TIMEOUT` if reached.
Adaptive capping uses this as the ceiling for individual run runtimes.

---

**Dominance** (FocusedILS)
In the current implementation, configuration θ₁ dominates θ₂ when it has been evaluated
at at least the same fidelity and has a strictly lower aggregate score. Ties do not count
as improvements — this is what lets the fidelity grow when the incumbent survives a tie
instead of being replaced endlessly by equal-scoring challengers.

The acceptance criterion is the one exception: it resolves ties in favour of the challenger,
so the ILS home base can cross plateaus and drift away from a basin it cannot improve on
while the incumbent stays put. ParamILS spells the two variants
`dominates(θ₁, θ₂, equalIsBetter)`.

---

**Home base** (ILS)
The local optimum the next perturbation starts from — ParamILS's `last_ils_state`. Distinct
from the incumbent: the incumbent is the best configuration found, the home base is where the
search currently is. It is replaced by the new local optimum whenever that one is at least as
good, and, like the incumbent, is re-measured whenever the fidelity grows. Replacements are
logged as `ils: new home base:` with the parameter diff against the previous one. Only the home
base is perturbed, so a home base that stops moving turns the ILS into repeated random restarts
from a fixed ball however the incumbent behaves — which is what the log lets you check.

---

**Fidelity**
The number of leading training instances used to score each configuration. FocusedILS starts
at `initial_fidelity` and grows by `fidelity_step` when the incumbent survives a challenge.
Instances are taken in list order and are not shuffled.

---

**Forbidden combination**
A joint assignment of parameter values that is excluded from the search.
Declared in the `.params` file as `{param1=val1, param2=val2}`.
Any configuration containing a forbidden combination is skipped during neighbourhood
exploration.

---

**Incumbent**
The best configuration found so far during the ILS run.
Updated whenever a new configuration has a strictly lower aggregate score at the required
fidelity. The final incumbent is returned as the result.

---

**Instance**
A benchmark problem on which the target algorithm is evaluated.
Passed as a file path to the solver wrapper.
RamParILS evaluates configurations across the training instance set to estimate
generalisation performance.

---

**Iterated Local Search (ILS)**
The search algorithm at the core of RamParILS.
Alternates between *local search* (greedy improvement within the neighbourhood) and
*perturbation* (random escape from a local optimum).
See [Algorithm](algorithm.md) for a full description.

---

**Local optimum**
A configuration whose entire neighbourhood contains no strictly better configuration.
ILS escapes local optima via perturbation rather than accepting them as the final answer.

---

**Neighbourhood**
The set of all configurations that differ from the current configuration in exactly one
parameter value.  Local search explores the neighbourhood at each step, evaluating all
neighbours in parallel.

---

**Objective**
What the tuner is trying to optimise.
`run_obj: runtime` minimises the mean (or median) solver runtime across instances;
`run_obj: quality` minimises the mean (or median) numeric cost returned by the solver.
See also: *overall objective*.

---

**Overall objective** (`overall_obj`)
How per-instance results are aggregated into a single scalar for comparison.
`mean` is sensitive to all instances including outliers; `median` is more robust but
ignores magnitude differences.

---

**Parameter space**
The set of all configurations defined by the `.params` file: parameter names, discrete
domains, defaults, conditional activations, and forbidden combinations.
RamParILS searches this space to find a good configuration.

---

**Perturbation**
A random walk of `perturbation_strength` steps applied to the current local optimum to
escape it and seed the next local search.  Each step randomly changes one parameter to
a uniformly sampled value from its domain.  Larger values jump further in the space.

---

**Pruning** (see also: *adaptive capping*)
Shorthand for adaptive capping: heuristic early termination when a candidate's partial
mean exceeds its configured bound. Enabled by default (`pruning: true`).

---

**Run objective** (`run_obj`)
What a single solver invocation measures: `runtime` (wall-clock seconds) or `quality`
(a scalar cost returned by the solver). Both objectives are minimised.

---

**Solver wrapper**
A script or executable that invokes the target algorithm with a given instance and
parameter setting, then prints a result line in RamParILS format.
See [Solver protocol](protocol.md) for the exact interface.

---

**Strategy**
Synonym for *configuration*, commonly used in the context of automated reasoning solver
portfolios (e.g., Grackle).  A strategy is a complete parameter setting that defines
the solver's behaviour.

---

**Strategy hash**
A compact fingerprint (64-bit integer) of the active configuration used as part of the
cache key. It is computed from the sorted active `param=value` pairs, so inactive
conditional values do not create duplicate cache entries. The hash is not guaranteed
to remain portable across Rust versions.

---

**Tuner timeout** (`tuner_timeout`)
The total wall-clock budget for the RamParILS run in seconds.
Once elapsed, no new evaluations are started and the incumbent is returned.
Distinct from `cutoff_time`, which limits individual solver runs.
