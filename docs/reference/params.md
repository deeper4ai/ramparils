# 🎛️ Parameter file format

Parameter files (`.params`) describe the configuration space: which parameters exist, their
domains, defaults, conditional activation, and forbidden combinations.

RamParILS supports the discrete parameter syntax used by the original Ruby
ParamILS implementation. Continuous ranges and other ParamILS variants are not
supported; enumerate every allowed value explicitly.

## 🔢 Discrete parameters

```
name {val1, val2, val3, ...} [default]
```

Example:

```
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189]
rho   {0, 0.17, 0.5, 1}                               [0.5]
```

Values are always strings. Numeric values are parsed by the target algorithm.

**The default must be a member of the domain.** A file whose default is not listed is rejected at
load time with the offending line, rather than silently starting somewhere unexpected:

```text
Error: line 4: default '0.03' not in domain ["0.0", "0.01", "0.05", "0.1", "0.2"] for param 'wp'
```

## 🔀 Conditional parameters

A conditional parameter is only *active* (included in the command line) when its parent has a
specific value:

```
child {val1, val2} [default] | parent in {allowed1, allowed2}
```

Example:

```
noise_type {random, walk} [random]
noise_param {0.0, 0.1, 0.5} [0.1] | noise_type in {walk}
```

`noise_param` is only passed to the algorithm when `noise_type = walk`. Otherwise it is omitted
from the command line.

Conditions resolve **transitively**: a parameter whose parent is itself conditional is inactive
whenever the parent is, so a chain `a -> b -> c` needs no restatement of `a` in `c`'s condition.

## ⛔ Forbidden combinations

A forbidden combination prevents specific joint assignments from being evaluated:

```
{param1=val1, param2=val2, ...}
```

Example:

```
{alpha=1.01, rho=0}
```

Any configuration where `alpha=1.01` **and** `rho=0` simultaneously is skipped during search.

## 💬 Comments

Lines starting with `#` and trailing `#...` are ignored:

```
# This is a comment
alpha {1.01, 1.189} [1.189]   # inline comment
```

## 🧩 Full example

```
# SAPS parameters
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189]
rho   {0, 0.17, 0.5, 1}                               [0.5]
ps    {0.0, 0.01, 0.05, 0.1, 0.2, 0.5}                [0.1]
wp    {0.0, 0.01, 0.03, 0.05, 0.1, 0.2}               [0.03]

# Forbidden: degenerate case
{alpha=1.01, rho=0}
```

---

## 🧠 Designing a space

The syntax above is the easy half. What follows is what a space costs to search, and it is the
part that decides whether a tuning run finds anything.

### A domain is a neighbourhood cost, paid every descent

The neighbourhood of a configuration is every configuration differing in exactly one parameter, so
a parameter contributes `|domain| - 1` neighbours to **every** step of every local search. A
five-valued parameter costs four; a boolean costs one. Perturbation draws from that neighbourhood
too, so the five-valued parameter is also perturbed four times as often — which looks like the
search finding it important when it is only finding it *sampled*. Do not read "parameters changed
by improving moves" as an importance ranking.

Trim a domain to the values that mean different things. Where a response is smooth and unimodal a
coarse ladder loses nothing; where it is a spike or non-monotone, resolution is load-bearing and
trimming can hide the optimum.

### Declare conditionals — they are free, and their absence is not

The cache key is a hash of the **active** configuration. Declaring `child | parent in {...}` is
therefore not documentation: it collapses every setting of an inactive child into **one** cache
entry. Leave the condition out and the search evaluates all of them, gets identical scores, and
reads the result as a plateau — the same symptom a dead parameter produces, and indistinguishable
from it without looking at the target algorithm's own statistics.

The rule that follows: **any option that gates whether another option is read must be declared as
that option's parent.** This includes gates that are not obviously conditional from the outside —
an option that skips constructing a component silently disables every option that component
consumes.

### Conditionals, not forbidden clauses, for combinations that merely mean something else

A forbidden combination removes a configuration from the space. Use it when the combination is
genuinely invalid or degenerate — when it duplicates another configuration reachable by a different
route, for instance. Use a **condition** when the combination is legal and runs, but makes the
child irrelevant. The two are not interchangeable: forbidding shrinks the space, conditioning
shrinks the space *and* the number of evaluations.

### Watch what is unreachable from the starting configuration

A parameter behind a guard that is off in the starting configuration cannot move until the search
first flips that guard — and the guard is flipped **alone**, with its dependents wherever they
happen to sit. If the guard does not pay at its dependents' defaults, a first-improvement descent
rejects it and the whole sub-space stays unreachable at any budget. Nothing in the search can
recover from that, because while the guard is off the cache has collapsed every setting of the
dependents to a single entry, so there is no information about them to learn from.

Practical consequences:

- prefer spaces in which everything is reachable from the starting configuration;
- when a guarded sub-space matters, evaluate it directly rather than hoping the search enters it —
  enumerating a small sub-cube offline is cheap and tells you whether it is bad or merely badly
  initialised;
- treat a parameter that is inactive at the default as far more expensive than its domain size
  suggests.

### Verify that a parameter does anything at all

A parameter that parses, reaches the command line, and changes nothing is the most expensive
mistake available here: it never errors, and the symptom — whole neighbourhoods scoring
identically — reads as a plateau. Before adding an option to a space, run the target algorithm at
two extreme values and confirm its own counters move. Structural checks cannot catch this: the
parameter is active by all of them.
