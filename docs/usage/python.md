# Python API

```python
import ramparils
```

The module exposes a single function.

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
        "algo":          "ruby /path/to/saps_wrapper.rb",
        "paramfile":     "/path/to/saps.params",
        "instances":     [
            "/path/to/instances/inst1.cnf",
            "/path/to/instances/inst2.cnf",
        ],
        "cutoff_time":   5.0,
        "tuner_timeout": 120.0,
    },
    cache_db="/tmp/ramparils_cache.db",
    cores=8,
)

print("Best config found:")
for k, v in sorted(result.items()):
    print(f"  {k} = {v}")
```

See `examples/saps_python.py` for a runnable example.
