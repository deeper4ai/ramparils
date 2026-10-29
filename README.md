# 🐏 RamParILS

**📖 Documentation: https://deeper4ai.github.io/ramparils/**

> Give it a parameter space, a set of instances, and a time budget. Get back a
> faster or lower-cost algorithm configuration.

RamParILS is a parallel Rust implementation of
[ParamILS](https://www.cs.ubc.ca/labs/algorithms/Projects/ParamILS/), an
automatic algorithm configurator based on Iterated Local Search. It evaluates
candidate configurations concurrently, keeps every result in a persistent
SQLite cache, and works with any command-line algorithm behind a small wrapper
protocol. It is the inner tuner of
[Grackle](https://github.com/ai4reason/grackle).

<p align="center">
  <img src="https://raw.githubusercontent.com/deeper4ai/ramparils/main/docs/figures/basic-ils.svg" width="100%"
       alt="Basic ILS: initialization, first local search, and the main loop">
</p>

## ⚙️ Install

```sh
pip install ramparils                                                              # Python extension
cargo install --git https://github.com/deeper4ai/ramparils --tag v0.3.0 --locked   # CLI
```

Details: [installation](https://deeper4ai.github.io/ramparils/installation.html).

## ⚡ Use

```sh
ramparils run scenario.yaml        # tune
ramparils db  results.dbcache      # export what the cache holds
```

A scenario names a [parameter file](https://deeper4ai.github.io/ramparils/reference/params.html),
the training instances, and a [wrapper](https://deeper4ai.github.io/ramparils/reference/protocol.html)
that runs the target algorithm. See the [CLI and scenario reference](https://deeper4ai.github.io/ramparils/usage/cli.html),
the [Python API](https://deeper4ai.github.io/ramparils/usage/python.html), and
the worked examples in [`examples/`](https://github.com/deeper4ai/ramparils/tree/main/examples) (E prover, primo, llm2smt).

## 🤖 LLM skills

[`skills/`](https://github.com/deeper4ai/ramparils/tree/main/skills) holds skills for LLM coding agents that walk through a
whole tuning campaign, from the parameter space to scoring the strategies
found. See [LLM skills](https://deeper4ai.github.io/ramparils/usage/skills.html).

## 🛠️ Development

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
./docs-build.sh        # ./docs-serve.sh to preview, ./docs-deploy.sh to publish
```

The E prover integration test needs `eprover` on `PATH`. Changes are recorded
in the [changelog](https://github.com/deeper4ai/ramparils/blob/main/CHANGELOG.md).

## 🤝 Acknowledgements

RamParILS is part of [DEEPER](https://deeper4ai.github.io/) and is supported by
the [DEEPER grant](https://www.renaissancephilanthropy.org/deeper-exploratory-engine-for-precise-expert-reasoning)
from Renaissance Philanthropy.
