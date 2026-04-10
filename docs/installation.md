# Installation

## Python extension (pip)

```bash
pip install ramparils
```

## CLI binary

Clone the repo and build with Cargo:

```bash
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
cargo build --release
# binary at target/release/ramparils
```

Requires [Rust 1.85+](https://rustup.rs).

## Python extension (from source)

```bash
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
pip install maturin
maturin develop --features python
```

## Verify

```python
import ramparils
help(ramparils.specialize)
```
