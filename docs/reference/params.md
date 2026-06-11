# 🎛️ Parameter file format

Parameter files (`.params`) describe the configuration space: which parameters exist, their
domains, defaults, conditional activation, and forbidden combinations.

RamParILS supports the discrete parameter syntax used by the original Ruby
ParamILS implementation. Continuous ranges and other ParamILS variants are not
supported; enumerate every allowed value explicitly.

## 🔢 Discrete parameters

```
name {val1, val2, val3, ...} [default]
```

Example:

```
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189]
rho   {0, 0.17, 0.5, 1}                               [0.5]
```

Values are always strings. Numeric values are parsed by the target algorithm.

## 🔀 Conditional parameters

A conditional parameter is only *active* (included in the command line) when its parent has a
specific value:

```
child {val1, val2} [default] | parent in {allowed1, allowed2}
```

Example:

```
noise_type {random, walk} [random]
noise_param {0.0, 0.1, 0.5} [0.1] | noise_type in {walk}
```

`noise_param` is only passed to the algorithm when `noise_type = walk`. Otherwise it is omitted
from the command line.

## ⛔ Forbidden combinations

A forbidden combination prevents specific joint assignments from being evaluated:

```
{param1=val1, param2=val2, ...}
```

Example:

```
{alpha=1.01, rho=0}
```

Any configuration where `alpha=1.01` **and** `rho=0` simultaneously is skipped during search.

## 💬 Comments

Lines starting with `#` and trailing `#...` are ignored:

```
# This is a comment
alpha {1.01, 1.189} [1.189]   # inline comment
```

## 🧩 Full example

```
# SAPS parameters
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189]
rho   {0, 0.17, 0.5, 1}                               [0.5]
ps    {0.0, 0.01, 0.05, 0.1, 0.2, 0.5}                [0.1]
wp    {0.0, 0.01, 0.05, 0.1, 0.2}                     [0.03]

# Forbidden: degenerate case
{alpha=1.01, rho=0}
```
