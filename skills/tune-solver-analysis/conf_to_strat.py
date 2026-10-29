#!/usr/bin/env python3
"""Turn RamParILS configurations into solverpy strategy files.

Imports the wrapper and calls its own translation function, so a strategy is
exactly the command line the wrapper would run. Input files are
`ramparils db confs` exports (or YAML blocks cut from a debug log): one
`name: value` per line.

    conf_to_strat.py --wrapper my_wrapper.py --function build_strategy \
        --out solverpy_db/strats confs/<stem>/ram-*
    conf_to_strat.py ... --names names.txt   # lines "short-name conf-file-basename"

The function may return a string or a list of arguments. YAML is parsed by
hand on purpose: yaml.safe_load turns unquoted on/off/yes/no into booleans.
"""

import argparse
import importlib.util
import os
import sys


def load_function(path, name):
    sys.path.insert(0, os.path.dirname(os.path.abspath(path)))
    spec = importlib.util.spec_from_file_location("wrapper_under_export", path)
    if spec is None or spec.loader is None:
        sys.exit(f"cannot import wrapper {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return getattr(module, name)


def parse_conf(path):
    params = {}
    with open(path) as f:
        for line in f:
            line = line.split("#", 1)[0].rstrip()
            if not line.strip():
                continue
            key, sep, value = line.partition(":")
            if not sep:
                raise ValueError(f"{path}: not a 'key: value' line: {line!r}")
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
                value = value[1:-1]
            params[key.strip()] = value
    return params


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--wrapper", required=True)
    p.add_argument("--function", default="build_strategy")
    p.add_argument("--out", required=True)
    p.add_argument("--names", help="file of 'short-name conf-basename' lines; only these are written")
    p.add_argument("confs", nargs="+")
    args = p.parse_args()

    build = load_function(args.wrapper, args.function)
    by_base = {os.path.basename(c): c for c in args.confs}
    if args.names:
        with open(args.names) as f:
            jobs = [line.split() for line in f if line.strip() and not line.startswith("#")]
    else:
        jobs = [(base, base) for base in by_base]

    os.makedirs(args.out, exist_ok=True)
    for short, base in jobs:
        strategy = build(parse_conf(by_base[base]))
        if not isinstance(strategy, str):
            strategy = " ".join(strategy)
        with open(os.path.join(args.out, short), "w") as f:
            f.write(strategy + "\n")
    print(f"wrote {len(jobs)} strategies to {args.out}")


if __name__ == "__main__":
    main()
