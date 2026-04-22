# Python API

```python
import ramparils
```

The Python extension provides a single function, `specialize`, that runs FocusedILS from an
initial strategy and returns the best configuration found within the time budget.  It is a
native Rust extension built with PyO3, so there is no subprocess overhead — the ILS loop,
parallel evaluation, and SQLite cache all run in-process.  All tuning options are passed as
fields in the `scenario` dict, matching the YAML keys documented in the [CLI reference](cli.md).

## specialize

::: ramparils.specialize

## Example

```python
import ramparils

result = ramparils.specialize(
    strategy={
        "alpha": "1.189",
        "rho":   "0.5",
        "ps":    "0.1",
        "wp":    "0.03",
    },
    scenario={
        # Required
        "algo":          "ruby /path/to/saps_wrapper.rb",
        "paramfile":     "/path/to/saps.params",
        "instances":     [
            "/path/to/instances/inst1.cnf",
            "/path/to/instances/inst2.cnf",
        ],
        "cutoff_time":   5.0,
        "tuner_timeout": 120.0,
        # Optional — defaults shown
        "cache_db":      "/tmp/ramparils_cache.db",
        "cores":         0,       # 0 = all available
        "approach":      "focused",
        "debug":         False,
    },
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
```

See `examples/saps_python.py` for a runnable example.
