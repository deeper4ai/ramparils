#!/usr/bin/env python3

"""
Example: run RamParILS from Python on the E prover / bushy010 benchmark.

Run from any directory:
    python examples/eprover/run.py
"""

import os
from pathlib import Path
from solverpy_grackle.runner.eprover import EproverRunner

import ramparils
import strategies

HERE = Path(__file__).parent.resolve()

eprover = EproverRunner({
   "domain1": "solverpy_grackle.trainer.eprover.default.DefaultDomain",
   "timeout": 1,
})

# The grackle-eprover wrapper looks up problem files via this env var.
os.environ["SOLVERPY_BENCHMARKS"] = str(HERE / "bushy010")

result = ramparils.specialize(
    strategy=strategies.default,
    scenario={
        "algo":          str(HERE / "grackle-eprover.sh"),
        "paramfile":     str(HERE / "params-eprover.txt"),
        "instance_file": str(HERE / "instances-bushy010.txt"),
        "cutoff_time":   1.0,
        "tuner_timeout": 600.0,
        "run_obj":       "quality",
        "overall_obj":   "mean",
        "cache_db":      str(HERE / "eprover-bushy010.dbcache"),
        "cores":         20,
        "debug_log":     str(HERE / "ramparils.log"),
    },
)


print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
print(eprover.args(result))
