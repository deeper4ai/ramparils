# ⚙️ Installation

## 📦 Python extension (pip)

```bash
pip install ramparils
```

## 🦀 CLI binaries

Clone the repository and install both command-line tools:

```bash
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
cargo install --path . --locked
```

This installs `ramparils` and `ramparils-db` into Cargo's binary directory,
normally `~/.cargo/bin`. Make sure that directory is on `PATH`.

For a repository-local build instead:

```bash
cargo build --release --locked
./target/release/ramparils --help
./target/release/ramparils-db --help
```

Both methods require [Rust 1.85+](https://rustup.rs).

## 🛠️ Python extension (from source)

```bash
git clone https://github.com/deeper4ai/ramparils.git
cd ramparils
pip install maturin
maturin develop
```

`maturin develop` installs the extension into the active Python environment.
Using a virtual environment is recommended.

## ✅ Verify

Verify the Python package:

```bash
python -c "import ramparils; help(ramparils.specialize)"
```

Verify a Cargo installation:

```bash
ramparils --help
ramparils-db --help
```

The Python package requires Python 3.9 or newer. The command-line tools do not
require Python.
