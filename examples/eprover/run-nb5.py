#!/usr/bin/env python3

"""
Example: run RamParILS from Python on the E prover / bushy010 benchmark.

Starts from the strategy given by the defaults in params-eprover-nb5.txt.

Run from any directory:
    python examples/eprover/run-nb5.py
"""

import os
import json
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

# Starting strategy: defaults from params-eprover-nb5.txt.
result = ramparils.specialize(
    strategy=strategies.nb5,
    scenario={
        "algo":          str(HERE / "grackle-eprover.sh"),
        "paramfile":     str(HERE / "params-eprover-nb5.txt"),
        "instance_file": str(HERE / "instances-bushy010.txt"),
        "cutoff_time":   1.0,
        "tuner_timeout": 600.0,
        "run_obj":       "quality",
        "overall_obj":   "mean",
        "cache_db":      str(HERE / "eprover-bushy010.dbcache"),
        "cores":         20,
        "debug_log":     str(HERE / "ramparils-nb5-py.log"),
    },
)


print("Best config found:")
print(json.dumps(result, indent=2, sort_keys=True))
print("Final E Prover strategy:")
print(eprover.args(result))
