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
ramparils --scenariofile scenario.yaml
```

The working directory matters because the scenario refers to the wrapper,
parameter file, instance list, cache, and logs using relative paths.

By default the wrapper runs `primo` from `PATH`. Override the executable when
needed:

```bash
PRIMO=/path/to/primo ramparils --scenariofile scenario.yaml
```

Each solver run has a 4096 MiB virtual-memory limit by default. The limit is
applied with `RLIMIT_AS` and inherited by solver subprocesses. Override it with
`PRIMO_MEMORY_MB`, for example:

```bash
PRIMO_MEMORY_MB=8192 ramparils --scenariofile scenario.yaml
```

When developing RamParILS locally, run the same scenario through Cargo:

```bash
cd examples/primo
cargo run --release --manifest-path ../../Cargo.toml -- \
  --scenariofile scenario.yaml
```

## ⏱️ Scope

This is a pipeline smoke test, not a tuning campaign. Every bundled instance
solves in well under 20 ms, so the runtime objective separates configurations by
measurement noise rather than by merit. Treat the reported incumbent as evidence
that the wrapper, parameter space, and scenario fit together — point
`instance_file` at harder QF_LRA input before drawing any conclusion about which
options actually help.

The scenario here starts FocusedILS at four instances per configuration, raises
fidelity by four when the incumbent survives a challenge, and stops after 300
seconds. Dropping `initial_fidelity` and `fidelity_step` to 1 and
`tuner_timeout` to 45 turns it into a run that finishes inside a minute, which
is enough to exercise every path in the wrapper.

## 🎛️ Parameter space

`params-primo.txt` covers primo's QF_LRA theory options and the SAT/DPLL(T)
boundary. The QF_UF/EUF family is deliberately left out: it is inert on this
benchmark set. So are the QF_UFLRA Nelson-Oppen knobs (`--nelson-oppen-prop`,
`--model-equality-branching`, `--model-equality-branch-budget`,
`--model-equality-branch-policy`), which do nothing unless a formula reaches
the mixed EUF/LRA solver.

Nine parameters are conditional, matching what primo's own `--help` and its
simplex implementation say about when each option takes effect:

- `lra_row_propagation` is active only while `lra_propagation` is on.
- `lra_bidirectional_row_propagation` and the forward row-size/fanout caps are
  active only while `lra_row_propagation` is on.
- The reverse-direction caps are active only while
  `lra_bidirectional_row_propagation` is on.
- `lra_least_violated_leaving` and `lra_sparse_pricing_candidates` are active
  only under `lra_pivoting_rule = sparse`, the only rule that consults the
  sparse leaving-side and pricing heuristics.
- `lra_sparse_leaving` hangs off `lra_least_violated_leaving` rather than off
  the pivoting rule: the leaving-variable loop in `LraTableau::check` tests
  least-violated first and skips the short-row branch entirely when it fires,
  so the two are not independent. The chain keeps it inactive under every
  non-sparse rule too.
- The Bland fallback point (`lra_bland_fallback_factor`,
  `lra_bland_fallback_offset`) is active under every rule except `bland`, which
  uses Bland selection from the first pivot anyway.

RamParILS resolves these transitively and omits inactive parameters from the
command line, so the wrapper looks every name up defensively and falls back to
primo's built-in default when one is absent.

One forbidden combination is declared: `lra_bland_fallback_factor = 0` together
with `lra_bland_fallback_offset = 0` falls back on the first pivot, which only
reproduces `lra_pivoting_rule = bland` by another route.

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
