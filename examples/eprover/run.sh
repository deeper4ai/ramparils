# Tune E prover on the ten bundled bushy problems.
#
# Everything the tuner needs is in scenario.yaml -- cutoff, budget, objective,
# cache and debug logging. Before v0.1.2 some of those were CLI flags, and
# before the CLI was unified this read `ramparils --scenariofile scenario.yaml`.

SOLVERPY_BENCHMARKS=$PWD/bushy010 ramparils run scenario.yaml "$@"
