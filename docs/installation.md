# Installation

## Prerequisites

- **Rust 1.75+** — install via [rustup](https://rustup.rs)
- **Python 3.9+** and `maturin` (Python extension only)

## CLI binary

```bash
cargo build --release
# binary at target/release/parils
```

## Python extension

Install [maturin](https://github.com/PyO3/maturin), then build and install into the current
Python environment:

```bash
pip install maturin
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop --features python
```

Or build a wheel for distribution:

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin build --features python --release
pip install target/wheels/parils-*.whl
```

!!! note "Python 3.13+"
    `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` is required on Python 3.13+ until PyO3 0.22 is
    released.

## Verify

```python
import parils
help(parils.specialize)
```
