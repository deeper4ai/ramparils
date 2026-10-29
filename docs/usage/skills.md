# 🤖 LLM skills

RamParILS ships a set of **skills** for LLM coding agents: instruction files
that teach an agent how to run a whole tuning campaign with RamParILS, from
building the parameter space to scoring the strategies it finds. They are
plain Markdown (one `SKILL.md` per directory, with a short front-matter
description the agent uses to decide when to load it), in the
[`skills/`](https://github.com/deeper4ai/ramparils/tree/main/skills)
directory of the repository.

They encode what our own tuning campaigns on ATP and SMT solvers taught us:
which scenario settings work, which mistakes silently waste a run, and why
the final configuration a run prints is not necessarily the best one.

## 📚 The skills

| Skill | What it covers |
|---|---|
| [`tune-solver`](https://github.com/deeper4ai/ramparils/blob/main/skills/tune-solver/SKILL.md) | The entry point: the campaign loop (domain → wrapper → benchmark split → scenario → smoke run → run → analysis → evaluation → refine), scenario settings that have worked, budget and determinism, and an experiment diary with predictions written before each run |
| [`tune-solver-domain`](https://github.com/deeper4ai/ramparils/blob/main/skills/tune-solver-domain/SKILL.md) | Building a parameter space from the solver's help and source, or by extending an older one; verifying values; conditionals; dead and inert parameters; compacting coupled options (E's clause-selection heuristics as the example) |
| [`tune-solver-wrapper`](https://github.com/deeper4ai/ramparils/blob/main/skills/tune-solver-wrapper/SKILL.md) | Writing a wrapper that follows the [solver protocol](../reference/protocol.md): PAR1, `UNKNOWN`, `runhash`, `--version`/`--params`/`--clean`, limits and process cleanup. Includes a standard-library `wrapper_template.py` |
| [`tune-solver-analysis`](https://github.com/deeper4ai/ramparils/blob/main/skills/tune-solver-analysis/SKILL.md) | Reading the debug and error logs, exporting the cache with `ramparils db`, choosing strategies worth evaluating (incumbents, greedy cover), finding inert parameters from runhash collisions, minimising a configuration. Includes `greedy_cover.py` and `conf_to_strat.py` |
| [`tune-solver-eval`](https://github.com/deeper4ai/ramparils/blob/main/skills/tune-solver-eval/SKILL.md) | Scoring strategies on a test benchmark with [SolverPy](https://github.com/ai4reason/solverpy): anchors and duplicate nulls in every batch, the noise floor, cached vs. measured results, train vs. test, portfolio value |

The four specialised skills also work on their own — for example, to review a
parameter file or to score a set of strategies that were not tuned by
RamParILS at all.

## ⚙️ Installing

Agents look for skills in a fixed directory. For Claude Code, that is
`~/.claude/skills/` for skills available in every project, or
`.claude/skills/` inside one project:

```bash
git clone https://github.com/deeper4ai/ramparils.git
mkdir -p ~/.claude/skills
cp -r ramparils/skills/* ~/.claude/skills/
```

Copy them again after a `git pull` to pick up changes. Other agents that read
the same `SKILL.md` format take the same directories, copied to wherever that
agent looks for skills.

The skills refer to this documentation and to the worked examples in
[`examples/`](https://github.com/deeper4ai/ramparils/tree/main/examples)
(`examples/eprover/`, `examples/primo/`). Keep the clone around and tell the
agent where it is, or let it read them from GitHub and from this site.

## 💬 Using them

The agent loads a skill by itself when a request matches its description, so
asking in plain words is usually enough:

```text
Tune the E prover on the problems in bench/train.txt with RamParILS; the best
strategy we have so far is in strategies/best.txt.
```

```text
Review params-mysolver.txt before we start a long run.
```

```text
The run in runs/03 has finished. Analyse it and pick the strategies to
evaluate on bench/test.
```

To name a skill explicitly in Claude Code, type it as a command:
`/tune-solver`, `/tune-solver-analysis`, and so on.

What to expect from a session started with `tune-solver`:

1. **Questions first.** The goal (deployment cutoff, single best strategy or
   a portfolio), the anchor strategy every evaluation is compared to, and the
   training and test benchmarks.
2. **Files to review before anything long runs** — the parameter file, the
   wrapper, the scenario — then a smoke run of a few minutes, with its log
   header and error log checked.
3. **A prediction note written before the long run**, and a diary entry after
   it that checks each prediction against the result.
4. **After the run**, not just the final configuration: every incumbent and a
   set of complementary strategies, converted to strategy files and scored on
   the test benchmark against the anchor.
5. **A proposal for the next round**: what to drop from or add to the domain,
   which configuration to start from, what to change in the scenario.

Long runs, remote machines and anything that costs real compute stay under
your control: tell the agent what it may launch on its own, and where.
