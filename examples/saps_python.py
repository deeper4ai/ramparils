"""
Example: run RamParILS from Python on the SAPS/SWGCP benchmark.

Expects the SAPS wrapper and instance data in the paths below.
Adjust paths to match your local setup before running.
"""

import ramparils

result = ramparils.specialize(
    strategy={
        "alpha": "1.189", "rho": "0.5", "ps": "0.1", "wp": "0.03"
    },
    scenario={
        "algo":      "ruby /path/to/saps_wrapper.rb",
        "paramfile": "/path/to/saps-params.txt",
        # Pass instances as a list directly — or use "instance_file" for a path.
        "instances": [
            "/path/to/instances/inst1.cnf",
            "/path/to/instances/inst2.cnf",
            "/path/to/instances/inst3.cnf",
        ],
        "cutoff_time":   5.0,
        "tuner_timeout": 30.0,
    },
    cache_db="/tmp/ramparils_saps_cache.db",
    cores=4,
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
