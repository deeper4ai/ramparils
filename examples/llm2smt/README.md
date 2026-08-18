# 🧪 llm2smt tuning example

This example tunes the `llm2smt` QF_EUF solver without `solverpy`. The wrapper
uses only the Python standard library and translates RamParILS parameter pairs
directly to the solver's command-line options.

The included `instances.txt` lists all 48 bundled SMT-LIB 2 problems. Paths are
interpreted from the directory in which RamParILS is started.

Run the example from this directory:

```bash
cd examples/llm2smt
ramparils run scenario.yaml
```

The working directory matters because the scenario refers to the wrapper,
parameter file, instance list, cache, and logs using relative paths.

By default the wrapper runs `llm2smt` from `PATH`. Override the executable when
needed:

```bash
LLM2SMT=/path/to/llm2smt ramparils run scenario.yaml
```

Each solver run has a 4096 MiB virtual-memory limit by default. The limit is
applied with `RLIMIT_AS` and inherited by solver subprocesses. Override it with
`LLM2SMT_MEMORY_MB`, for example:

```bash
LLM2SMT_MEMORY_MB=8192 ramparils run scenario.yaml
```

When developing RamParILS locally, run the same scenario through Cargo:

```bash
cd examples/llm2smt
cargo run --release --manifest-path ../../Cargo.toml -- \
  run scenario.yaml
```

The scenario starts FocusedILS at four instances per configuration and raises
fidelity by four instances when the incumbent survives a challenge. Adjust
`initial_fidelity`, `fidelity_step`, time limits, and `cores` for the benchmark.

The complete starting configuration is declared inline under `initial_config`.
This includes `nnf_memo`, although that parameter is inactive while `nnf` is
`false`. To keep it in a separate file instead, move the mapping to a YAML file
and replace `initial_config` with `initial_config_file: "initial-config.yaml"`.

The final complete configuration is printed as YAML and also written to
`ramparils.log`. Each improved incumbent in that log includes `hash=<hash>`
and is followed by its complete YAML configuration.

The wrapper reports `sat` and `unsat` as successful. `unknown`, process errors,
and timeouts receive the full cutoff runtime and a large quality penalty, so a
fast failed run cannot win the runtime objective.
