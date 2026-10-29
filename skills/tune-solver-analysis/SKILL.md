---
name: tune-solver-analysis
description: Analyse a RamParILS tuning run after (or during) it - read the debug log and error log, export and inspect the .dbcache with `ramparils db`, pick strategies worth evaluating (incumbent trajectory, greedy cover for complementary strategies), turn configurations into solver strategy files, detect inert parameters with runhash collisions, and minimise a configuration. Use when a run has finished or when deciding what to evaluate and how to change the domain next.
---

# After-run analysis

**The run's final configuration is not necessarily the best strategy it
produced.** The tuning score is measured on the training subset at the tuning
cutoff and rises monotonically by construction; the test-benchmark score of
the same incumbent sequence does not. And a mean-runtime tuner never
optimises for complementarity, though the cache is full of it. So analysis
ends with a *set* of candidates to evaluate (`tune-solver-eval`), never with
"the answer".

Work on copies: copy the `.dbcache` only after the RamParILS process has
exited (a copy of a live WAL-mode SQLite file came back corrupt twice), then
check it with `sqlite3 run.dbcache 'PRAGMA integrity_check'`.

## 1. Read the log

Useful lines in the debug log (`debug_log`):

| line | meaning |
|---|---|
| header (`binary:`, `git:`, `wrapper --version:` block, scenario echo) | provenance: what actually ran |
| `ils: initial config:` + its score | compare with the previous run's final when restarting from it — a mismatch means something changed (solver build) |
| `ils: new incumbent: hash=... score=... runhash=...` | the incumbent trajectory, each followed by its full YAML |
| `ils: bls improvement neighbor` / `ils: bls arguments` | accepted moves and which parameter changed |
| `ils: bls local optimum score=` | end of a descent — but printed the same when the deadline cut the descent off; check timing |
| `ils: new home base` | acceptance (including `acceptance_tolerance` sideways moves) |
| `ils: random restart` | restart (stagnation / probability) |
| `capped`, gated rounds | partial scores; **never quote a capped or gated local-optimum score** |
| `ils: summary rounds= searched= gated= incumbents= evals= capped=` | the outcome line |
| `ils: final config:` | the final incumbent, complete (inactive parameters included) |

Questions to answer from it:

- When did the last improving incumbent appear (fraction of budget)?
- Did any descent finish, or did the whole budget go into one? (A long run
  once made 26 accepted steps and never restarted — the neighbourhood was too
  big for the budget.)
- Which parameters moved, how often, and in which accepted steps? Group them
  (e.g. heuristic slots vs. core options) — if one group never changes, or
  changes only in one coordinate (frequency but never CEF), compact it
  (`tune-solver-domain`).
- **Round trips**: a parameter moved A→B→A within one descent gives a lower
  bound on the objective's noise, from the log alone.
- A move **accepted alone** with an improvement **below that noise** is not
  evidence the parameter matters.

## 2. Read the error log

`error_log` holds every `UNKNOWN` run with its command and output. Classify
before acting:

- **domain bugs** — the solver rejecting a value (`Option ... requires ...`,
  unknown name). Fix the domain; every move toward that value was guaranteed
  to fail, so that part of the space was unreachable;
- **wrapper bugs** — shell syntax errors, missing files, unparsable output;
- **infrastructure** — missing model files, disk, memory limit;
- **solver bugs** — segfaults, internal errors, silent exits with no status.
  Keep the instance and the exact command for a bug report; rerun before
  calling it a bug.

## 3. Export the cache

```bash
ramparils db run.dbcache --out-dir export/solverpy_db
```

writes, per strategy hash `ram-<hash>`, under `<out-dir>/<kind>/<cache-stem>/`:

- `solved/` — solved instance paths, one per line;
- `status/` — `instance <TAB> status <TAB> runtime`;
- `confs/` — the **active** configuration as YAML (inactive parameters are
  absent: this is what ran, not a complete configuration — it cannot be fed
  back as `initial_config` unless the wrapper defaults fill the gaps);
- `runhashes/<cache-stem>.txt` — `sid combined-hash n-instances`.

Do not run `ramparils db` from inside its output directory (the path nests).
Exports of different caches never collide (namespaced by cache stem), so they
can be merged into one tree — check with
`comm -12 <(cd a; find . -type f | sort) <(cd b; find . -type f | sort)`
first, and merge via a staging directory.

Only fully evaluated strategies are comparable; capped ones have partial
`status` files. With `approach: basic`, compare strategies that have a row
for every training instance.

## 4. Choose candidates to evaluate

Always include:

- the **initial configuration** (the anchor, re-measured in the same batch);
- **every incumbent** (`init`, `inc1`, `inc2`, ..., `final`) — the best on the
  test benchmark is often an intermediate one;
- the **greedy cover** of the cache's solved sets: the strategy solving most,
  then the one adding most beyond it, and so on (`greedy_cover.py` beside this
  file). These are the portfolio candidates.

Pitfalls seen:

- greedy cover sees solved *sets*, not scores; a configuration tied with the
  final on solved count may be picked instead of it — add the final by hand;
- snapshotting incumbents from a run still in progress and then labelling them
  as the run's results; regenerate from the concluded log;
- a solved count on a 500-instance subset under-predicts the full pool's
  ordering in the fine detail — evaluate a few more cover members than you
  think you need.

## 5. Configuration → strategy file

Evaluation (solverpy) needs the solver's command-line options, not a
parameter assignment. **Generate them with the wrapper's own translation
function**, never with a re-implementation, so the strategy cannot drift from
what ran:

```bash
python3 conf_to_strat.py --wrapper path/to/my_wrapper.py --function build_strategy \
    --out solverpy_db/strats export/solverpy_db/confs/<cache-stem>/ram-*
```

`conf_to_strat.py` (beside this file) parses the `key: value` YAML **by hand**:
`yaml.safe_load` turns unquoted `on`/`off`/`yes`/`no` into booleans, and a
parameter whose domain is literally `{auto, on, off}` then renders as
`True`. It also accepts a `--names` file to give short names
(`run3-inc2 ram-<hash>`) instead of hashes.

To extract the incumbents from a log, take the YAML block after each
`ils: new incumbent` / `ils: final config:` line; a config recovered from the
log is complete, one from `confs/` is active-only — both render the same
through a wrapper that defaults missing names.

## 6. Inert parameters from runhash collisions

If the wrapper emits a runhash, strategies with the **same combined runhash**
did byte-identical search work on every instance. Group them and look at
which parameters differ inside each group:

- a parameter that varies only inside collision groups never changes
  behaviour on this benchmark — a removal candidate; confirm in the source
  (globally dead, only read on another code path, or conditionally inert);
- a parameter that is inert only when another one has a certain value points
  to a missing conditional.

Two precautions:

1. **Restrict to instances every compared strategy solved** before combining.
   An instance that timed out can still carry a hash and differ by where the
   clock cut it off; unrestricted XORs of truly identical strategies then
   disagree. (RamParILS's own wrapper protocol only emits runhash on success,
   which mostly avoids this; other sources may not.)
2. **Measure the solver's run-to-run agreement first.** If identical reruns
   disagree on a large share of instances (E with address-space
   randomisation did), runhash collisions cannot be trusted until the
   nondeterminism is removed (`setarch -R`).

Collisions show up when restarts converge to the same basin with an inert
dimension left at its random draw — so runs with several restarts give the
most information.

## 7. Minimise

A tuned configuration usually carries passengers: options accepted early that
contribute nothing. Do backward elimination of its departures from the
default: evaluate the configuration with each departure reverted, drop the
one whose removal costs least (or helps), repeat — O(k²) evaluations, not
O(2^k). In one campaign a nine-option configuration reduced to two options
with the same score, and the long tail of the tuning run itself had been
doing this slowly. Score minimisation candidates on the test benchmark, not
the tuning objective. When the remaining options are few, enumerate the full
cube instead (`tune-solver-domain`).

## 8. Write it down

Record in the diary (`tune-solver`): the summary line, the trajectory, error
log classes and fixes, exported strategy counts, which candidates go to
evaluation and why, and each prediction checked. Keep analysis outputs as
timestamped files under `data/`.
