# ramparils --debug --debug-log ramparils.log --cachedb ":memory:" --scenariofile scenario.yaml $@

SOLVERPY_BENCHMARKS=$PWD/bushy010 ramparils --debug --debug-log ramparils.log --cachedb eprover-bushy010.dbcache --scenariofile scenario.yaml $@
