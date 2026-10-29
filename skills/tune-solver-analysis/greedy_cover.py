#!/usr/bin/env python3
"""Greedy set cover over solved-instance files.

Each input file lists solved instances, one per line (`ramparils db solved`
or solverpy `solved/` output). Prints strategies in greedy order: the one
solving most, then the one adding most beyond the union so far, and so on.

    greedy_cover.py solverpy_db/solved/<stem>/ram-* [--top N] [--min-gain K]
"""

import argparse
import os


def read(paths):
    world = {}
    for path in paths:
        if os.path.isfile(path):
            with open(path) as f:
                world[os.path.basename(path)] = {line.strip() for line in f if line.strip()}
    return world


def cover(world, top, min_gain):
    union = set()
    rest = dict(world)
    order = []
    while rest and len(order) < top:
        name = max(rest, key=lambda s: (len(rest[s] - union), len(rest[s]), s))
        gain = len(rest[name] - union)
        if gain < min_gain:
            break
        union |= rest.pop(name)
        order.append((name, len(world[name]), gain, len(union)))
    return order


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("files", nargs="+")
    p.add_argument("--top", type=int, default=20)
    p.add_argument("--min-gain", type=int, default=1)
    args = p.parse_args()
    world = read(args.files)
    print(f"# {'strategy':<24} {'alone':>7} {'gain':>7} {'union':>7}")
    for name, alone, gain, total in cover(world, args.top, args.min_gain):
        print(f"{name:<26} {alone:>7} {gain:>7} {total:>7}")


if __name__ == "__main__":
    main()
