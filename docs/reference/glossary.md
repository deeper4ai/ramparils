# Glossary

---

**Active parameter**
A parameter that is included in the solver invocation for a given configuration.
Conditional parameters are only active when their parent parameter has the required value;
inactive parameters are omitted from the command line entirely.

---

**Adaptive capping** (see also: *pruning*)
An early-stopping rule that abandons evaluation of a configuration once its accumulated
runtime exceeds `bound_multiplier × incumbent_runtime`.  At that point the configuration
cannot possibly beat the incumbent, so further evaluation is wasteful.  Controlled by the
`pruning` and `bound_multiplier` scenario fields.

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
Configuration θ₁ *dominates* θ₂ when θ₁ has been evaluated on at least as many instances
as θ₂ and achieves equal or better performance on every one of them.
FocusedILS accepts a new configuration only if it can dominate the incumbent on the
instances evaluated so far, avoiding unnecessary full evaluations of poor candidates.

---

**Forbidden combination**
A joint assignment of parameter values that is excluded from the search.
Declared in the `.params` file as `{param1=val1, param2=val2}`.
Any configuration containing a forbidden combination is skipped during neighbourhood
exploration.

---

**Incumbent**
The best configuration found so far during the ILS run.
Updated whenever a new configuration is found that dominates (FocusedILS) or outperforms
(BasicILS) the current incumbent.  The final incumbent is returned as the result.

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
`run_obj: quality` maximises the mean (or median) solution value returned by the solver.
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
Shorthand for adaptive capping: early termination of a configuration's evaluation when
it is provably worse than the incumbent.  Enabled by default (`pruning: true`).

---

**Run objective** (`run_obj`)
What a single solver invocation measures: `runtime` (wall-clock seconds) or `quality`
(a scalar value returned by the solver).  Determines how the solver wrapper's result line
is interpreted and how configurations are compared.

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
A compact fingerprint (64-bit integer) of a configuration used as a cache key.
Computed from the sorted `param=value` pairs.  Two configurations with the same active
parameters and values always produce the same hash.

---

**Tuner timeout** (`tuner_timeout`)
The total wall-clock budget for the RamParILS run in seconds.
Once elapsed, no new evaluations are started and the incumbent is returned.
Distinct from `cutoff_time`, which limits individual solver runs.
