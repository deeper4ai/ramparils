# Solver wrapper protocol

Parils communicates with the target algorithm via a simple subprocess protocol.

## Invocation

The solver is invoked as a shell command:

```
<algo> <instance> <cutoff_time> -param1 val1 -param2 val2 …
```

- `<algo>` — the command from the scenario's `algo` field (passed to `sh -c`)
- `<instance>` — path to the instance file
- `<cutoff_time>` — per-run time limit in seconds
- `-param val` pairs — active parameters in alphabetical order

Example:

```
ruby /path/to/saps_wrapper.rb /data/inst1.cnf 5.0 -alpha 1.189 -rho 0.5 -ps 0.1 -wp 0.03
```

## Output

The solver must print a result line to **stdout**:

```
#%# PARILS #%# <status>, <runtime>, <quality>
```

| Field | Values | Description |
|-------|--------|-------------|
| `status` | `OK`, `TIMEOUT`, `CRASHED` | Outcome of the run |
| `runtime` | float | Actual runtime in seconds (capped to `cutoff_time`) |
| `quality` | float | Solution quality (used when `run_obj: quality`) |

The result line may appear anywhere in stdout — other output is ignored.
If no result line is found, the run is treated as a timeout: runtime is set to `cutoff_time`.

## Examples

Successful run:

```
#%# PARILS #%# OK, 1.23, 0.0
```

Timeout:

```
#%# PARILS #%# TIMEOUT, 5.0, 0.0
```

## Minimal wrapper skeleton

```ruby
#!/usr/bin/env ruby
instance, cutoff, *params = ARGV

# ... run your solver here ...

puts "#%# PARILS #%# OK, #{runtime}, 0.0"
```
