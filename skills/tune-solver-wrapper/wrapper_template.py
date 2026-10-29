#!/usr/bin/env python3
"""RamParILS wrapper template (standard library only).

Copy, rename, and fill in the parts marked FILL IN. The protocol is in
docs/reference/protocol.md; the reasons behind each rule are in the
tune-solver-wrapper skill.

    wrapper INSTANCE CUTOFF [-name value ...]   run one solve, print the result line
    wrapper --version                            wrapper + solver version, supports: line
    wrapper --params [-name value ...]           print the solver command line, run nothing
    wrapper --clean  [-name value ...]           print only active, non-default parameters
"""

from __future__ import annotations

import hashlib
import os
import platform
import re
import resource
import signal
import subprocess
import sys
import time
from pathlib import Path

WRAPPER_VERSION = "0.1.0"
SUPPORTS = ["version", "runhash", "params", "clean"]

BINARY = os.environ.get("SOLVER_BINARY", "mysolver")  # FILL IN
MEMORY_MB = int(os.environ.get("SOLVER_MEMORY_MB", "4096"))
NO_ASLR = os.environ.get("SOLVER_NO_ASLR", "1") == "1"

FAILURE_QUALITY = 10_000_000.0

# FILL IN: the solver's success statuses (TPTP SZS or SMT-LIB vocabulary).
SUCCESS = {"Theorem", "Unsatisfiable", "Satisfiable", "CounterSatisfiable", "ContradictoryAxioms"}
# sat/unsat for SMT

# FILL IN: every parameter of the .params file -> the solver's own default.
# Inactive conditional parameters are not passed by RamParILS, so every
# lookup falls back to this table.
PARAM_DEFAULTS: dict[str, str] = {
    "sel": "SelectMaxLComplexAvoidPosPred",
    "presat": "0",
    "fwdemod": "2",
    "slots": "0",
    "heur1": "1",
}

# Fixed constants that are deliberately not tuned (say where they came from).
FIXED_OPTIONS: list[str] = []

# Example of an encoded family: heurN is an index into fixed (freq, CEF) pairs.
HEURISTIC_PAIRS = [
    ("1", "ConjectureRelativeSymbolWeight(PreferGround,0.5,100,100,100,100,1.5,1.5,1)"),
    ("2", "ConjectureRelativeSymbolWeight(PreferGround,0.5,100,100,100,100,1.5,1.5,1)"),
    ("1", "FIFOWeight(PreferProcessed)"),
]
MAX_SLOTS = 1


def is_active(name: str, params: dict[str, str]) -> bool:
    """Mirror the .params file's conditionals. FILL IN."""
    if name.startswith("heur"):
        return int(params.get("slots", PARAM_DEFAULTS["slots"])) >= int(name[4:])
    return True


def build_options(params: dict[str, str]) -> list[str]:
    """The only place parameters become solver flags. FILL IN."""
    unknown = set(params) - set(PARAM_DEFAULTS)
    if unknown:
        raise ValueError(f"unknown parameters: {sorted(unknown)}")
    p = {**PARAM_DEFAULTS, **params}
    opts = list(FIXED_OPTIONS)

    # always-emitted value option
    opts.append(f"--literal-selection-strategy={p['sel']}")
    # boolean switch: flag only when on
    if p["presat"] == "1":
        opts.append("--presat-simplify")
    # value option with a "solver default" that means: omit the flag
    if p["fwdemod"] != PARAM_DEFAULTS["fwdemod"]:
        opts.append(f"--forward-demod-level={p['fwdemod']}")
    # encoded family, expanded here (values in the .params file stay shell-safe)
    slots = int(p["slots"])
    if not 0 <= slots <= MAX_SLOTS:
        raise ValueError(f"slots out of range: {slots}")
    if slots:
        items = []
        for i in range(1, slots + 1):
            freq, cef = HEURISTIC_PAIRS[int(p[f"heur{i}"])]
            items.append(f"{freq}*{cef}")
        items.append("1*FIFOWeight(ConstPrio)")
        opts.append(f"--define-heuristic=({','.join(items)})")
    return opts


def parse_output(stdout: str) -> tuple[str, dict[str, int]]:
    """Return (status, deterministic counters). FILL IN for the solver."""
    m = re.search(r"SZS status (\w+)", stdout)
    status = m.group(1) if m else "UNKNOWN"
    stats = {k: int(v) for k, v in re.findall(r"^# (\w[\w ]*?)\s*:\s*(\d+)$", stdout, re.M)}
    return status, stats


# Counters that differ between identical runs; never hash them. FILL IN.
NOISY_COUNTERS: set[str] = set()


def runhash(stats: dict[str, int]) -> str | None:
    keep = sorted((k, v) for k, v in stats.items() if k not in NOISY_COUNTERS)
    if not keep:
        return None
    text = ";".join(f"{k}={v}" for k, v in keep)
    return hashlib.blake2b(text.encode(), digest_size=8).hexdigest()


def solver_command(instance: str, cutoff: float, opts: list[str]) -> list[str]:
    """FILL IN: how the solver takes options, a time limit and the instance."""
    cmd = [BINARY, *opts, f"--cpu-limit={int(cutoff) + 1}", instance]
    if NO_ASLR:
        cmd = ["setarch", platform.machine(), "-R", *cmd]
    return cmd


def limit_memory() -> None:
    limit = MEMORY_MB * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (limit, limit))


def solve(instance: str, cutoff: float, opts: list[str]) -> tuple[str, float, float, str]:
    cmd = solver_command(instance, cutoff, opts)
    start = time.monotonic()
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,  # own process group, so a timeout kills everything
        preexec_fn=limit_memory,
    )
    try:
        stdout, stderr = proc.communicate(timeout=cutoff)
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        proc.communicate()
        return "Timeout", cutoff, FAILURE_QUALITY, ""  # a real timeout: cacheable
    elapsed = time.monotonic() - start
    status, stats = parse_output(stdout)
    if status == "UNKNOWN":
        print(f"wrapper: no status, exit {proc.returncode}\n{stdout[-2000:]}\n{stderr[-2000:]}", file=sys.stderr)
    if status in SUCCESS:
        rh = runhash(stats)
        return status, min(elapsed, cutoff), 0.0, f", {rh}" if rh else ""
    return status, cutoff, FAILURE_QUALITY, ""  # PAR1 for every non-success


def parse_parameters(args: list[str]) -> dict[str, str]:
    if len(args) % 2:
        raise ValueError("parameters must be -name value pairs")
    params = {}
    for name, value in zip(args[::2], args[1::2]):
        if not name.startswith("-") or name == "-":
            raise ValueError(f"bad parameter name {name!r}")
        params[name.lstrip("-")] = value
    return params


def print_version() -> int:
    print(f"{Path(sys.argv[0]).name} {WRAPPER_VERSION}")
    ok = True
    try:
        out = subprocess.run([BINARY, "--version"], capture_output=True, text=True, check=True).stdout
        print(out.rstrip("\n"))
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"wrapper error: {error}", file=sys.stderr)
        print(f"{BINARY} MISSING")
        ok = False
    print(f"supports: {' '.join(SUPPORTS)}")
    return 0 if ok else 1


def print_clean(params: dict[str, str]) -> None:
    kept = {k: v for k, v in sorted(params.items()) if is_active(k, params) and v != PARAM_DEFAULTS.get(k)}
    print(" ".join(f"-{k} {v}" for k, v in kept.items()))


def main() -> int:
    argv = sys.argv[1:]
    try:
        if argv[:1] == ["--version"]:
            return print_version()
        if argv[:1] == ["--params"]:
            print(" ".join(build_options(parse_parameters(argv[1:]))))
            return 0
        if argv[:1] == ["--clean"]:
            print_clean(parse_parameters(argv[1:]))
            return 0
    except ValueError as error:
        print(f"wrapper error: {error}", file=sys.stderr)
        return 1
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2

    instance, cutoff = argv[0], 0.0
    try:
        cutoff = float(argv[1])
        opts = build_options(parse_parameters(argv[2:]))
        status, runtime, quality, rh = solve(instance, cutoff, opts)
    except (OSError, ValueError, KeyError, IndexError) as error:
        print(f"wrapper error: {error}", file=sys.stderr)
        status, runtime, quality, rh = "UNKNOWN", cutoff, FAILURE_QUALITY, ""
    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}{rh}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
