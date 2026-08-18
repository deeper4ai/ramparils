# 🧪 primo tuning example

This example tunes the `primo` QF_LRA solver without `solverpy`. The wrapper
uses only the Python standard library and translates RamParILS parameter pairs
directly to the solver's command-line options.

The included `instances.txt` lists all 21 bundled SMT-LIB 2 problems, every one
of them QF_LRA. Paths are interpreted from the directory in which RamParILS is
started.

Run the example from this directory:

```bash
cd examples/primo
ramparils run scenario.yaml
```

The working directory matters because the scenario refers to the wrapper,
parameter file, instance list, cache, and logs using relative paths.

By default the wrapper runs `primo` from `PATH`. Override the executable when
needed:

```bash
PRIMO=/path/to/primo ramparils run scenario.yaml
```

Each solver run has a 4096 MiB virtual-memory limit by default. The limit is
applied with `RLIMIT_AS` and inherited by solver subprocesses. Override it with
`PRIMO_MEMORY_MB`, for example:

```bash
PRIMO_MEMORY_MB=8192 ramparils run scenario.yaml
```

When developing RamParILS locally, run the same scenario through Cargo:

```bash
cd examples/primo
cargo run --release --manifest-path ../../Cargo.toml -- \
  run scenario.yaml
```

## ⏱️ Scope

This is a pipeline smoke test, not a tuning campaign. Every bundled instance
solves in well under 20 ms, so the runtime objective separates configurations by
measurement noise rather than by merit. Treat the reported incumbent as evidence
that the wrapper, parameter space, and scenario fit together — point
`instance_file` at harder QF_LRA input before drawing any conclusion about which
options actually help.

The scenario here runs **BasicILS**: every configuration is scored on all 21
instances, so there is no fidelity schedule to reason about and scores are
directly comparable. It stops after 300 seconds. Dropping `tuner_timeout` to 45
turns it into a run that finishes inside a minute and still exercises every
path — a 45 s run reaches 20 local optima, a new incumbent, four home-base
moves and one stagnation restart, so the acceptance and restart machinery is
covered rather than merely configured.

Its tuning knobs are the ones that came out of a nine-run campaign on the full
SMT-LIB QF_LRA division, rescaled to this space's 24 parameters, and
`scenario.yaml`'s header comment says which to adjust for a real campaign and
why — `cutoff_time` and `instance_file` first, then `cores`, which defaults to
every logical core and usually should not.

`approach: focused` is the alternative, and it is deliberately not the default
here. **It is much less exercised than `basic`**: the two campaign runs that
used it predate the fidelity-consistency fix in v0.1.3, and no long run has
used it since, so today it rests on `tests/focused_fidelity.rs` rather than on
a campaign. The scenario file shows what to set, and keeps `initial_fidelity`
and `fidelity_step` present but inert under `basic`.

## 🎛️ Parameter space

`params-primo-qflra.txt` covers primo's QF_LRA theory options, its Boolean
flattening thresholds and the SAT/DPLL(T) boundary. It was revised on
2026-08-18 against a nine-run tuning campaign, a 512-configuration ablation and
some ninety further configurations evaluated on the full SMT-LIB QF_LRA
division; the `+N` / `-N` figures in its comments are that evidence. They are recorded, not acted on: **no option is
excluded for scoring badly**, because one benchmark at one cutoff on one solver
build is not grounds for deciding an option is useless everywhere. Options come
out only when another parameter in the file already covers them, and the
numeric ladders are coarser than before but span the same ranges.

The QF_UF/EUF family is deliberately left out: with no uninterpreted functions
the EUF atoms are Boolean constants, so congruence closure has no applications
to merge (`--euf-engine lazy` measured +1 instance, `--euf-explain-search
structural-proof-forest` +0). The Nelson-Oppen options
(`--nelson-oppen-prop`, `--model-equality-branch-budget`,
`--model-equality-branch-policy`) are out for the same reason.

**`--model-equality-branching` is out for a different reason, and an earlier
version of this README had it wrong.** It is not inert on pure QF_LRA: it is a
control-flow switch. `check_sat.cpp:10420` skips constructing
`OnlineLraPropagator` when it is set, with no else branch, so `lra_model_phase`,
`lra_propagation`, `lra_row_propagation`, `lra_theory_decisions` and the
row-propagation caps are then parsed, placed on the command line, and never
read. It measured -97 instances. A space that wants it must declare it as a
*guard* over all of those, not as a forbidden clause: the combination is legal
and runs, it just means something else, and the guard is what collapses the
configurations differing only in now-inactive parameters to a single cache
entry instead of a plateau of identical scores.

Ten parameters are conditional, matching what primo's own `--help` and its
simplex implementation say about when each option takes effect:

- `lra_row_propagation` is active only while `lra_propagation` is on.
- `lra_bidirectional_row_propagation` and the row-size/fanout caps are active
  only while `lra_row_propagation` is on.
- `lra_soi_minimize` is active only under `lra_pivoting_rule = soi`, and
  `lra_soi_minimize_order` only while the minimizer is `deletion`.
- `lra_least_violated_leaving` and `lra_sparse_pricing_candidates` are active
  only under `lra_pivoting_rule = sparse`, the only rule that consults the
  sparse leaving-side and pricing heuristics.
- `lra_sparse_leaving` hangs off `lra_least_violated_leaving` rather than off
  the pivoting rule: the leaving-variable loop in `LraTableau::check` tests
  least-violated first and skips the short-row branch entirely when it fires,
  so the two are not independent. The chain keeps it inactive under every
  non-sparse rule too.
- The Bland fallback point (`lra_bland_fallback_factor`) is active under every
  rule except `bland`, which uses Bland selection from the first pivot anyway.

RamParILS resolves these transitively and omits inactive parameters from the
command line, so the wrapper looks every name up defensively and falls back to
primo's built-in default when one is absent.

No forbidden combinations are declared. The previous version needed one —
`lra_bland_fallback_factor = 0` together with `lra_bland_fallback_offset = 0`
falls back on the first pivot and so only reproduces `lra_pivoting_rule =
bland` — and dropping the offset retired it.

### What came out, and why

Three parameters were removed. Each is redundant against something else in the
file, which is a different claim from "it did not help".

- **`lra_bland_fallback_offset`** is the constant term of the same threshold as
  `lra_bland_fallback_factor` (`factor * variables + offset`), and is dominated
  by the first term on any tableau with more than a handful of variables. With
  it pinned at primo's 64, `factor = 0` becomes a legitimate distinct setting —
  a flat 64-pivot fallback — instead of a second route to `bland`, so the
  forbidden clause went with it.
- **`lra_bidirectional_row_propagation_max_row_size`** and
  **`-max_fanout`** were the only parameters three guards deep, so a search
  reaches them only after two moves that each have to pay for themselves first;
  and they widen the same propagation the forward caps already widen, along a
  response measured to be monotone in cap width. Two knobs on one monotone axis
  is one knob. primo's defaults (32 / 32) apply.

Four parameters were added, all new primo options the wrapper already covered.
`boolean_flatten_threshold` and `boolean_flatten_post_threshold` are described
below; `lra_soi_minimize` and `lra_soi_minimize_order` are in the space for a
structural reason rather than a hopeful one. With `soi` listed as a pivoting
rule and its knobs pinned, a search could only ever evaluate `soi` at their
defaults, so a sub-space that might pay would be unreachable at any budget.
Because the cache key is the *active* configuration, a guarded parameter costs
nothing until its guard opens.

`top_level_or_tseitin` folds two primo flags into one three-valued parameter:
`auto` (the default automatic rule, no flag), `always`
(`--top-level-or-tseitin`), and `never` (`--no-top-level-or-tseitin`). primo
also has `--no-auto-top-level-or-tseitin`, which needs no parameter of its own:
with the forced-on flag absent it does exactly what `never` does.

`guarded_real_equality_lowering` and `monotone_elimination` are preprocessing
rewrites that primo groups with its QF_LRA options. Both are unconditional and
off by default, so the wrapper passes their flags only when the tuner turns
them on.

`mixed_dispatch` is a routing switch rather than a heuristic: `wide` sends a
QF_LRA formula containing a propositional variable to the mixed EUF/LRA solver,
`narrow` (the default) keeps it on the pure path. primo documents `wide` as
usually much worse but genuinely better on some instances, which is what makes
it worth a portfolio dimension.

### The Boolean flattening pair

`boolean_flatten_threshold` and `boolean_flatten_post_threshold` bound the arity
up to which nested and/or nodes are flattened, before and after the rewriting
preprocessing passes respectively. The pair exists because preserving shared
Boolean DAGs stopped the term constructors flattening on construction — right
for memory, but it cost QF_LRA up to 2.7x until the post-pass was added, since
the rewriting passes rebuild nesting the pre-pass has already gone past. The
pre-pass is off by default (`0`); the post-pass defaults to `2048`. Note that
`0` disables rather than unbounds, so it is the value that reintroduces the
regression, not a neutral one.

On the QF_LRA division the post-pass is the most live single option in this
file: disabling it costs 20 instances and 21.8% more core runtime. `0` is kept
in both domains all the same, since preserving shared Boolean DAGs is exactly
what flattening gives up, and on a formula with a very long and/or chain that
trade goes the other way — the synthetic case that motivated the DAG change
went 1.88 GB to 57 MB.

`lra_soi_minimize` and `lra_soi_minimize_order` select how a multi-row SOI
conflict is shrunk before it becomes a clause. **Both are inert unless
`lra_pivoting_rule` is `soi`, and the order is inert unless the minimizer is
`deletion`**, which is how they are declared here. primo's own measurements
find the minimization worth having and the ordering within noise, but on
clause-length proxies rather than runtime, which is the gap a tuning run can
close.

## 📤 Output

The complete starting configuration is declared inline under `initial_config`.
It lists the conditional parameters too, even though several are inactive at the
default setting.

The final complete configuration is printed as YAML and also written to
`ramparils.log`. Each improved incumbent in that log includes `hash=<hash>`
and is followed by its complete YAML configuration.

The wrapper reports `sat` and `unsat` as successful. `unknown`, process errors,
and timeouts receive the full cutoff runtime and a large quality penalty, so a
fast failed run cannot win the runtime objective.
