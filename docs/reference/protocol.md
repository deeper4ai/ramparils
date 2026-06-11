# 🔌 Solver wrapper protocol

RamParILS communicates with the target algorithm via a simple subprocess protocol.

## 📥 Invocation

The solver is invoked as a shell command:

```
<algo> <instance> <cutoff_time> -param1 val1 -param2 val2 …
```

- `<algo>` — the command from the scenario's `algo` field (passed to `sh -c`)
- `<instance>` — path to the instance file
- `<cutoff_time>` — per-run time limit in seconds
- `-param val` pairs — active parameters in alphabetical order

The complete command is passed to `sh -c`. Scenario files and parameter values
must therefore be trusted, and wrappers should avoid paths or values that need
shell quoting.

Example:

```
ruby /path/to/saps_wrapper.rb /data/inst1.cnf 5.0 -alpha 1.189 -rho 0.5 -ps 0.1 -wp 0.03
```

## 📤 Output

The solver must print a result line to **stdout**:

```
#%# RamParIls #%# <status>, <runtime>, <quality>
```

| Field | Values | Description |
|-------|--------|-------------|
| `status` | text | Outcome of the run; stored verbatim in the cache |
| `runtime` | float | Actual runtime in seconds (capped to `cutoff_time`) |
| `quality` | float | Numeric cost to minimise when `run_obj: quality` |

The result line may appear anywhere in stdout; other output is ignored.
RamParILS stores `status` for reporting but does not use it when scoring.
The wrapper must therefore encode timeouts, crashes, and unsolved cases as an
appropriate runtime or quality penalty.

If no valid result line is found, the status becomes `UNKNOWN`, runtime is set
to `cutoff_time`, and quality is set to `10000000`. `UNKNOWN` results are not
written to the persistent cache.

## 🧪 Examples

Successful run:

```
#%# RamParIls #%# OK, 1.23, 0.0
```

Timeout:

```
#%# RamParIls #%# TIMEOUT, 5.0, 0.0
```

## 🦴 Minimal wrapper skeleton

```ruby
#!/usr/bin/env ruby
instance, cutoff, *params = ARGV

# ... run your solver here ...

puts "#%# RamParIls #%# OK, #{runtime}, 0.0"
```
