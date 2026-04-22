# Algorithm

RamParILS implements **Iterated Local Search (ILS)** for automated algorithm configuration.
The goal is to find the parameter setting of a target algorithm that minimises runtime (or
maximises solution quality) on a set of training instances.

---

## Iterated Local Search

ILS alternates between two phases: **local search** finds a local optimum in the configuration
space, and **perturbation** escapes it by applying a random walk.  Starting from an initial
configuration (typically the parameter file's declared defaults), local search explores the
neighbourhood one parameter at a time, greedily accepting any neighbor that improves performance.
When no improving neighbor exists, perturbation applies `perturbation_strength` random steps to
escape the local optimum, and the cycle repeats until the tuner timeout expires.

The neighbourhood of a configuration is all configurations that differ in exactly one parameter.
For a space with P parameters and average domain size D, each configuration has at most (D−1)×P
neighbors; in practice the neighbourhood is evaluated in parallel across all available cores,
so the wall-clock cost per local search step is roughly `cutoff_time` regardless of width.

---

## BasicILS vs FocusedILS

The `approach` field selects the ILS variant.

**BasicILS** (`approach: basic`) evaluates every candidate configuration on exactly N
`(instance, seed)` pairs before comparing it to the incumbent.  This is simple but wasteful:
a clearly inferior configuration still receives N full evaluations before being rejected.

**FocusedILS** (`approach: focused`, the default) uses *adaptive dominance-based comparison*.
A new configuration is compared to the incumbent after each run: if it cannot dominate the
incumbent on the instances evaluated so far, evaluation stops immediately.  Configuration θ₁
dominates θ₂ when θ₁ has been run on at least as many instances as θ₂ and achieves equal or
better performance on all of them.  In practice this means poor configurations are filtered after
one or two runs while promising ones receive the full evaluation budget — typically 5–20× fewer
total solver calls than BasicILS for the same search quality.

**Random** (`approach: random`) samples configurations uniformly at random rather than following
a local search trajectory.  Useful as a baseline or when the parameter space has no exploitable
structure.

---

## Adaptive capping

Adaptive capping (controlled by `pruning` and `bound_multiplier`) stops evaluating a configuration
early if its running cost already exceeds `bound_multiplier × incumbent_cost`.  For runtime
optimisation, once a configuration has accumulated runtime `≥ bound_multiplier × best_runtime`,
it cannot possibly beat the incumbent and evaluation halts immediately.  This is
*trajectory-preserving*: the same local optima would be found without capping, just more slowly.

A lower `bound_multiplier` (e.g., 2.0) prunes more aggressively and speeds up search, but may
occasionally discard a configuration that would have been competitive on later instances.
The default of 10.0 is conservative and safe for most use cases; tighten it only if your cutoff
times are long and you have many instances.  Set `pruning: false` to disable capping entirely.

---

## Iterative deepening

Iterative deepening (`iterative_deepening: true`) runs ILS in multiple phases with an exponential
schedule rather than a single run.  Early phases use a small fraction of the training instances
and a short per-run cutoff — just enough to quickly filter the search space and find a good
starting point.  Later phases gradually increase the instance count, cutoff time, and per-phase
budget, refining the best region found so far.  The incumbent from each phase seeds the next,
so early exploration and late refinement share information.

Three growth factors control the schedule:

| Field | Controls | Effect of larger value |
|-------|----------|------------------------|
| `lambda_n` | fraction of instances used in the first phase | more instances early → slower but more accurate initial filter |
| `lambda_c` | fraction of `cutoff_time` used in the first phase | longer per-run cutoff early → more accurate but slower phases |
| `lambda_t` | fraction of `tuner_timeout` given to the first phase | more time early → deeper first-phase search |

All three default to `0.5`, giving a geometric doubling schedule.  Iterative deepening is most
useful when the training set is large (hundreds of instances) and the cutoff time is long (tens
of seconds), making a full-budget single run prohibitively slow at the start.
