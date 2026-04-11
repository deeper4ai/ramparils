"""
Example: run RamParILS from Python on the E prover / bushy010 benchmark.

Run from any directory:
    python examples/eprover/run.py
"""

import os
import sys
from pathlib import Path

import ramparils

HERE = Path(__file__).parent.resolve()

# The grackle-eprover wrapper looks up problem files via this env var.
os.environ["SOLVERPY_BENCHMARKS"] = str(HERE / "bushy010")

# Default strategy (all params at their defaults from params-eprover.txt).
strategy = {
    "condense":            "0",
    "defcnf":              "24",
    "der":                 "none",
    "ext_sup_max_depth":   "-1",
    "fool_unroll":         "true",
    "forwardcntxtsr":      "0",
    "fwdemod":             "2",
    "heur0":               "0",
    "heur1":               "1",
    "heur2":               "2",
    "heur3":               "3",
    "ho_order_kind":       "lfho",
    "inverse_recognition": "false",
    "lift_lambdas":        "true",
    "local_rw":            "false",
    "neg_ext":             "off",
    "no_eq_unfolding":     "0",
    "pos_ext":             "off",
    "prefer":              "0",
    "presat":              "0",
    "replace_inj_defs":    "false",
    "satcheck":            "none",
    "sel":                 "SelectMaxLComplexAvoidPosPred",
    "simparamod":          "none",
    "slots":               "4",
    "sos_input_types":     "0",
    "splaggr":             "0",
    "splcl":               "0",
    "srd":                 "0",
    "strong_rw_inst":      "0",
    "tord":                "LPO4",
    "tord_const":          "0",
    "tord_prec":           "arity",
    "tord_weight":         "arity",
}

result = ramparils.specialize(
    strategy=strategy,
    scenario={
        "algo":          str(HERE / "grackle-eprover.sh"),
        "paramfile":     str(HERE / "params-eprover.txt"),
        "instance_file": str(HERE / "instances-bushy010.txt"),
        "cutoff_time":   1.0,
        "tuner_timeout": 10.0,
        "run_obj":       "quality",
        "overall_obj":   "mean",
    },
    cache_db=str(HERE / "eprover-bushy010.dbcache"),
    cores=4,
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
