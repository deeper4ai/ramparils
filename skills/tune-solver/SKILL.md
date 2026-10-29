---
name: tune-solver
description: Invent new strategies for a solver (ATP, SMT, SAT, any command-line algorithm) by automatic configuration with RamParILS. Covers the whole campaign - parameter domain, wrapper, scenario file, the run/analyse/refine loop, evaluation on a test benchmark with solverpy, and the experiment diary. Use when asked to tune a solver, find better or complementary strategies, or set up a RamParILS run.
---

# Tune a solver with RamParILS

RamParILS searches a discrete parameter space with iterated local search and
returns the best configuration it found on a training benchmark. A **tuning
campaign** is never one run: it is a loop of runs, analyses and domain
refinements, and the result is a set of strategies scored on a separate
benchmark, not the number a run printed at the end.

This skill is the overview and the loop. The parts that can be used on their
own have their own skills:

| Part | Skill | When on its own |
|---|---|---|
| Parameter space: build, verify, compact, prune | `tune-solver-domain` | reviewing or extending an existing `.params` file |
| Wrapper: the RamParILS solver protocol | `tune-solver-wrapper` | wrapping a new solver or fixing a wrapper |
| After-run analysis: log, cache, strategy extraction | `tune-solver-analysis` | reading a finished or running RamParILS run |
| Evaluation on a test benchmark with solverpy | `tune-solver-eval` | scoring any set of strategies, tuned or not |

The reference docs live in this repository: `docs/usage/cli.md` (every
scenario field), `docs/reference/params.md` (parameter file and space design),
`docs/reference/protocol.md` (wrapper protocol), `docs/reference/algorithm.md`
(the search). `examples/eprover/` and `examples/primo/` are complete worked
setups. Read the relevant doc rather than guessing a field name — **neither
RamParILS nor solverpy rejects an unknown key**; a misspelled or misplaced key
parses and is silently ignored.

## The campaign loop

```
 0. diary + goal        what counts as better, on which benchmark, at which cutoff
 1. domain              tune-solver-domain
 2. wrapper             tune-solver-wrapper
 3. benchmark split     training subset for tuning, separate test set for scoring
 4. scenario            below
 5. smoke run           minutes, not hours; read the log header and error log
 6. prediction note     written BEFORE the long run starts
 7. run
 8. analysis            tune-solver-analysis
 9. evaluation          tune-solver-eval, on the test benchmark, with anchors
10. diary entry         results vs. prediction
11. refine              domain / wrapper / scenario / starting point -> back to 1 or 4
```

Stop when a run stops producing configurations that beat the anchor on the
test benchmark, or when the space has become small enough to enumerate (a few
dozen cells) — then measure every cell directly instead of searching it.

### 0. Decide what "better" means first

- **Deployment cutoff and benchmark.** Tune at, or near, the cutoff the
  strategies will be used at. A too-short cutoff makes the objective blind to
  options that only pay on longer runs — in one campaign an option worth ~4%
  more solved instances at 10 s was invisible to every 1 s run. A shorter
  cutoff buys many more descents per hour, so the usual compromise is a
  shorter cutoff for exploration and a confirmation at the real one.
- **Single best strategy or a portfolio?** A mean-runtime tuner finds one good
  configuration. Complementary strategies — ones that lose on the mean but
  solve what the best one misses — are often worth more than the tuning gain
  itself (a second solver or a losing-but-different arm repeatedly added more
  than the whole tuning campaign did). If a portfolio is the goal, plan to
  harvest diverse strategies from the cache (`tune-solver-analysis`), and
  prefer looser acceptance and random restarts that visit more basins.
- **Anchor.** Name the best known strategy (the solver's default, a
  hand-tuned strategy, an earlier tuned one). Every evaluation carries it.

### 3. Benchmark split

Tune on a **training subset**; score on instances the tuner never saw.

- Drop instances that every strategy times out on, and instances every
  strategy solves instantly — they cost compute and carry no ranking signal.
  In one cache over ~20k strategies, 36% of the instances were identical for
  every strategy; a 473-instance subset chosen by between-strategy variance
  kept 95% of the variance and rank-correlated 0.997 with the full set at 31%
  of the cost.
- For a large pool, stratify by status (solved by all / some / none) and pick
  representatives per stratum (e.g. k-means over instance features, take the
  real instance nearest each centroid). **Give each stratum a quota
  proportional to its share, not a fixed count** — a fixed floor for the
  "solved by all" stratum ate 80% of a small subset and destroyed the ranking.
- **Validate the subset before tuning on it**: score ~10 known strategies on
  the subset and on the full pool at the *same cutoff*, compare rank
  correlation (Spearman) and the per-strategy scaling error. Subset scores can
  be strongly rank-correlated with the full pool and still systematically
  mis-predict magnitude.
- A small subset (a few hundred) with `approach: basic` gives many complete
  descents in a day; a larger one gives fewer, better-founded steps.

### 4. Scenario file

Full field list: `docs/usage/cli.md`. A starting point that has worked for
ATP and SMT solvers on a few hundred to a couple of thousand instances:

```yaml
algo: "python3 my_wrapper.py"
paramfile: "params-mysolver.txt"
instance_file: "train-subset.txt"     # paths are relative to the cwd ramparils runs in
initial_config_file: "start.yaml"     # the anchor strategy, translated into the space
cutoff_time: 5.0
tuner_timeout: 43200                  # 12 h; see "budget" below

run_obj: runtime                      # wrapper reports PAR1 runtime
overall_obj: mean

approach: basic                       # every candidate scored on every instance
perturbation_strength: 7              # max(2, ceil(0.2 * number_of_parameters))
restart_failures: 5                   # restart after 5 rejected local optima
restart_target: incumbent             # restart near the best, not uniformly
restart_strength: 0                   # 0 = 2 * perturbation_strength
acceptance_tolerance: 0.01            # accept a slightly worse local optimum

pruning: true
bound_multiplier: 1.2

cores: 63                             # physical cores minus one, never logical
cache_db: "/fast/disk/mysolver.dbcache"
debug: true
debug_log: "/fast/disk/run.log"
error_log: "/fast/disk/run.errors"
```

**`approach`:**

- `basic` — every candidate on the whole instance list; scores are directly
  comparable. **The default choice for small benchmarks** (up to ~2,000
  instances at a few seconds' cutoff), and what nearly all productive runs
  used.
- `focused` — scores on a growing prefix, raises fidelity when the incumbent
  survives. For benchmarks too expensive to evaluate fully per candidate.
  Less exercised; watch the fidelity trajectory in the log. A full-fidelity
  round costs the same however you get there, and reachable fidelity grows
  only with the square root of `fidelity_step`.
- `random` — fresh random configuration after each local optimum, no
  acceptance criterion. A baseline, and a good **source of diverse strategies
  for a portfolio**; rarely the best single configuration. A uniform random
  draw is almost always far worse than the incumbent, so with pruning on most
  random rounds are gated at the entrance.
- `iterative_deepening: true` — geometric schedule of instances, cutoff and
  deadline; for very long runs on large sets.

**Escaping local optima** (calibrated over many runs):

- `acceptance_tolerance` 0.005–0.02. Too large disables restarts entirely
  (stagnation is never declared); 0 freezes the home base on any plateau.
  Calibrate against the spread of local-optimum scores in an earlier log.
- `restart_strength` around 4–8 steps. 10 of 22 parameters was a wrecking
  ball: the restart landed 20% worse and produced nothing for the rest of the
  run.
- `perturbation_strength` 1 cannot open a guard and retune its dependents in
  one move; 3+ can.

**Pruning:** `bound_multiplier` 1.2 is a reasonable default; 2.0 is almost no
pruning at short cutoffs; 1.05 is aggressive. A pruned ("capped") or gated
round's local-optimum score is a partial mean, not a real score — never
quote it. `futell` (checkpoint early rejection) is heuristic, can reject a
winner, and is off by default; validate it against a `futell: false` twin
before relying on it.

**Execution:**

- `cores` = physical cores, minus one for the tuner. **Never oversubscribe**:
  ~1.75x workers per physical core cost ~3% of solved instances and 2.7x the
  run-to-run variance for 41% less wall clock — strictly worse. `0` means all
  *logical* cores, which is usually wrong. Pin one thread per core if the
  machine is shared (`taskset -c 0-62` with `cores: 63`).
- `cache_db` on a fast local disk (NVMe/SSD). The SQLite cache serializes
  commits from all workers; on a RAID of spinning disks fsync latency made a
  run crawl. Not on tmpfs for long runs — a reboot or OOM loses it.
- **One cache per (solver build, wrapper semantics, cutoff semantics,
  parameter space).** The cache key is the active configuration and instance
  path only; nothing else. A stale cache silently serves old results.
  A cache of a failed launch (missing binary → everything "timed out") must be
  deleted, not reused.
- `debug_log` and `error_log` always. The error log is where bad domain values
  and crashes show up — read it during the smoke run.

**Budget:** most of the gain usually arrives early. Across nine runs the last
improving incumbent appeared at 2–30% of a 12 h budget, and the best
configuration of one campaign appeared in the first 15 minutes. Prefer more,
shorter, differently-seeded runs over one very long one — and evaluate the
whole incumbent trajectory, not just the end.

**Determinism:** RamParILS assumes a deterministic solver. Check how
deterministic it really is (rerun one strategy 3x and compare per-instance
results). For E, address-space randomization alone caused ~13% (and up to
~25–30%) per-instance run disagreement; launching under `setarch $(uname -m)
-R` removed it entirely. Solver randomness you cannot remove widens the noise
floor every comparison must clear.

### 5. Smoke run before every long run

Run the exact scenario with `tuner_timeout` of a few minutes and check:

- the log header: RamParILS `git:` revision (no `-dirty`), the wrapper's
  `--version` block naming the solver build you meant, instance count;
- the initial configuration's score is plausible — when restarting from a
  previous run's final on the same instances and build, it should reproduce
  that run's final score. **This is a free built-in control**: when it did
  not, the cause was a silent solver upgrade between runs;
- the error log is empty or explained;
- `ps` after the smoke run shows no leftover solver processes (leaked
  processes from earlier runs once cost a later measurement ~1% of instances).

### 7. Running

Launch detached (`nohup`, `tmux`, a systemd unit), log to file. Check in on
the log, not on stdout. Do not copy the `.dbcache` while the run is alive —
an rsync of a live WAL-mode SQLite file came back corrupt twice; copy after
the process has exited, and run `sqlite3 x.dbcache 'PRAGMA integrity_check'`.

### 11. Refining between runs

From the analysis, typical next moves:

- **domain**: drop parameters that never moved or were shown inert, fix values
  the error log rejected, compact independent pairs into fixed blocks, add a
  guard the source revealed, widen the one parameter that kept hitting a
  domain edge (`tune-solver-domain`);
- **starting point**: restart from the best *evaluated* configuration (not
  necessarily the last incumbent), or from a complementary strategy found in
  the cache to explore a different basin;
- **scenario**: cutoff, subset size, tolerance, restart settings;
- **minimise**: a final configuration usually carries passengers. Greedy
  backward elimination of its departures from the default (O(k²) evaluations)
  is far cheaper than an ablation cube and was repeatedly what the long tail
  of a run was doing by itself.

Never compare tuning scores across runs that differ in host, solver build,
cutoff, instance set, worker count or RamParILS version. Only a solverpy
evaluation in one batch settles which of two runs produced the better
strategy.

## The diary

Keep one `DIARY.md` per campaign, beside the scenario files. If the project's
own instructions say entries are written only on request, follow that and
offer the entry instead of writing it.

**Before each run — a prediction note** (`run-NN-<short-name>.md`), written
before launch, never after:

```markdown
# Run NN — <what is different from the previous run>

**Config:** scenario file, parameter file (+ checksum), initial config and where it came from.
**Why:** the question this run answers; what it is compared against.
**Predictions** (numbered, falsifiable):
1. first local optimum within ~N steps / by ~T minutes;
2. which parameters will move (and which should not);
3. expected tuning score range; expected test-benchmark result vs. the anchor;
4. what result would change the plan.
```

Predictions that fail teach the most — a recorded wrong prediction ("summing
main effects gives ~X", measured X+20) is what established that main effects
do not add. A prediction written after seeing the log is not a prediction.

**After each run — an analysis entry:**

- outcome line from the log (`ils: summary ...`), wall clock, whether the last
  descent reached a real local optimum or was cut by the deadline;
- incumbent trajectory (time, score, change), and which parameters moved;
- error-log findings and anything fixed in domain/wrapper as a result;
- which strategies were extracted for evaluation and why;
- evaluation results against anchors, with the batch's own noise floor;
- each prediction marked held / failed, with the number;
- decision for the next run.

**Provenance in every entry**: absolute timestamps with timezone, host and
working directory, launch command, scenario and parameter files, solver,
wrapper and RamParILS revisions (`--version` blocks from the log header),
benchmark and cutoff, core count, output paths. Keep observations from logs
apart from assumptions and from things someone said. Analysis output belongs
in timestamped files under `data/`, not pasted into the diary.

## Standing lessons

1. **The final configuration is not necessarily the best one.** Tuning score
   rose monotonically in every run; the test score of the incumbent sequence
   did not (one run: +52, +62, +63, then down to +57 at the final). Evaluate
   every incumbent.
2. **Score everything with solverpy before believing it.** Four of five tuned
   configurations of one campaign lost to the solver's default on the full
   benchmark; twelve hours of tuning from the best known strategy ended
   slightly below it.
3. **A parameter that parses and does nothing reads as a plateau, not an
   error.** Check every new parameter at two extreme values and confirm the
   solver's own statistics move (`tune-solver-domain`).
4. **Agreement between runs is evidence only if the effect clears the noise
   floor.** Three independent runs all chose an option worth nothing.
5. **Runs plateau early.** Spend saved budget on replicates, different
   starts and evaluation, not on longer runs.
6. **Portfolio beats single configuration.** Keep the cache; harvest it.
