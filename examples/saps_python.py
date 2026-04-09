"""
Example: run Parils from Python on the SAPS/SWGCP benchmark.

Run from the repo root:
    cd paramils
    python3 ../examples/saps_python.py
"""

import parils

result = parils.specialize(
    strategy={
        "alpha": "1.189", "rho": "0.5", "ps": "0.1", "wp": "0.03"
    },
    scenario={
        "algo":      "ruby example_saps/saps_wrapper.rb",
        "paramfile": "example_saps/saps-params.txt",
        # Pass instances as a list directly — or use "instance_file" for a path.
        "instances": [
            "example_data/SWGCP-satisfiable-instances/SWlin2006.10286.cnf",
            "example_data/SWGCP-satisfiable-instances/SWlin2006.19724.cnf",
            "example_data/SWGCP-satisfiable-instances/SWlin2006.2705.cnf",
            "example_data/SWGCP-satisfiable-instances/SWlin2006.24768.cnf",
            "example_data/SWGCP-satisfiable-instances/SWlin2006.27490.cnf",
        ],
        "cutoff_time":   5.0,
        "tuner_timeout": 30.0,
    },
    cache_db="/tmp/parils_saps_cache.db",
    cores=4,
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
