#!/usr/bin/env python3
"""RamParILS wrapper for the primo QF_LRA solver, via solverpy's `Primo`.

Translates a RamParILS `-name value ...` parameter list into a primo
command-line strategy and runs it through
`solverpy.solver.smt.primo.Primo`, which already supplies the time/memory
limits (`timeout`/`ulimit -Sv`, both applied to primo's whole process group,
so a RamParILS-level cancellation kills it too -- no signal handling needed
here), SMT status parsing, and the `runhash` fingerprint (see
`RunHash`/`PRIMO_RUNHASH_GEN`) used to detect inert parameters.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from solverpy.solver.smt.primo import PRIMO_BINARY, Primo

FAILURE_QUALITY = 10_000_000.0
DEFAULT_MEMORY_LIMIT_MB = 4096

WRAPPER_VERSION = "0.1.0"
# Features this wrapper answers `--version` with a `supports:` line for, per
# ramparils-primo-cont/IDEAS.md item 1. "clean" is not implemented yet (the
# `clean` note under "Lower priority, same protocol").
SUPPORTS = ["version", "runhash", "params"]

# Boolean parameters whose solver default is on: primo is told about them only
# to switch them off.
NEGATIVE_FLAGS = {
    "lra_propagation": "--no-lra-propagation",
    "lra_sparse_leaving": "--no-lra-sparse-leaving",
    "gaussian_elimination": "--no-gaussian-elimination",
}

# Boolean parameters whose solver default is off.
POSITIVE_FLAGS = {
    "lra_row_propagation": "--lra-row-propagation",
    "lra_bidirectional_row_propagation": "--lra-bidirectional-row-propagation",
    "lra_theory_decisions": "--lra-theory-decisions",
    "lra_model_phase": "--lra-model-phase",
    "lra_least_violated_leaving": "--lra-least-violated-leaving",
    "lra_fixed_elimination": "--lra-fixed-elimination",
    "monotone_elimination": "--monotone-elimination",
    "guarded_real_equality_lowering": "--guarded-real-equality-lowering",
}

# Parameters passed through as `--flag value`.
VALUE_OPTIONS = {
    "lra_row_propagation_max_row_size": "--lra-row-propagation-max-row-size",
    "lra_row_propagation_max_fanout": "--lra-row-propagation-max-fanout",
    "lra_bidirectional_row_propagation_max_row_size": (
        "--lra-bidirectional-row-propagation-max-row-size"
    ),
    "lra_bidirectional_row_propagation_max_fanout": (
        "--lra-bidirectional-row-propagation-max-fanout"
    ),
    "lra_pivoting_rule": "--lra-pivoting-rule",
    "lra_sparse_pricing_candidates": "--lra-sparse-pricing-candidates",
    "lra_bland_fallback_factor": "--lra-bland-fallback-factor",
    "lra_bland_fallback_offset": "--lra-bland-fallback-offset",
    "mixed_dispatch": "--mixed-dispatch",
    "pure_theory_filter": "--pure-theory-filter",
    "theory_phase": "--theory-phase",
    # Bounded Boolean flattening (primo f13a184/ac07b56). Two separate passes:
    # the first runs before the rewriting preprocessing passes and is off by
    # default, the second runs after them, because those passes build fresh
    # nodes through constructors that no longer flatten and so reintroduce
    # nesting the pre-pass has already gone past.
    "boolean_flatten_threshold": "--boolean-flatten-threshold",
    "boolean_flatten_post_threshold": "--boolean-flatten-post-threshold",
    # SOI conflict minimization (primo 2c3fc5c). Both are inert unless
    # lra_pivoting_rule is soi, and the order is inert unless the minimizer is
    # deletion, so a space that tunes them must pin the rule.
    "lra_soi_minimize": "--lra-soi-minimize",
    "lra_soi_minimize_order": "--lra-soi-minimize-order",
}

# Three states spread over two primo flags; "auto" is the solver default and
# needs no flag at all.
TSEITIN_FLAGS = {
    "auto": None,
    "always": "--top-level-or-tseitin",
    "never": "--no-top-level-or-tseitin",
}


def parse_parameters(arguments: list[str]) -> dict[str, str]:
    if len(arguments) % 2 != 0:
        raise ValueError("parameters must be supplied as -name value pairs")

    parameters: dict[str, str] = {}
    for index in range(0, len(arguments), 2):
        option = arguments[index]
        if not option.startswith("-") or option == "-":
            raise ValueError(f"invalid parameter name: {option!r}")
        parameters[option.lstrip("-")] = arguments[index + 1]
    return parameters


def is_true(value: str) -> bool:
    normalized = value.lower()
    if normalized not in {"true", "false"}:
        raise ValueError(f"expected true or false, got {value!r}")
    return normalized == "true"


def build_strategy(parameters: dict[str, str]) -> str:
    """Translate RamParILS parameters into a primo strategy string.

    The strategy is the space-joined primo command-line options, exactly what
    `Primo` appends after its static options (see
    `solverpy.solver.smt.primo.PRIMO_STATIC`) and before the instance.

    Inactive conditional parameters are omitted from the command line by
    RamParILS, so every lookup here has to tolerate a missing name and fall
    back to primo's own default.
    """
    options: list[str] = []

    for name, flag in NEGATIVE_FLAGS.items():
        if name in parameters and not is_true(parameters[name]):
            options.append(flag)

    for name, flag in POSITIVE_FLAGS.items():
        if name in parameters and is_true(parameters[name]):
            options.append(flag)

    for name, flag in VALUE_OPTIONS.items():
        if name in parameters:
            options.extend([flag, parameters[name]])

    if "top_level_or_tseitin" in parameters:
        mode = parameters["top_level_or_tseitin"]
        if mode not in TSEITIN_FLAGS:
            raise ValueError(f"unknown top_level_or_tseitin mode: {mode!r}")
        flag = TSEITIN_FLAGS[mode]
        if flag is not None:
            options.append(flag)

    return " ".join(options)


def memory_limit_giga() -> float:
    value = int(os.environ.get("PRIMO_MEMORY_MB", str(DEFAULT_MEMORY_LIMIT_MB)))
    if value <= 0:
        raise ValueError("PRIMO_MEMORY_MB must be positive")
    return value / 1024


def limit_string(cutoff: float, giga: float) -> str:
    """Build a solverpy limit string (`T<seconds>-M<gigabytes>`) from *cutoff*.

    RamParILS always calls the wrapper with a whole number of seconds (its
    fidelity schedule already rounds up via `.ceil()`), and `Primo`'s
    underlying `Limits` parser requires an integer `T`, so a fractional
    *cutoff* is rejected rather than silently truncated.
    """
    seconds = round(cutoff)
    if abs(cutoff - seconds) > 1e-6:
        raise ValueError(f"cutoff must be a whole number of seconds, got {cutoff!r}")
    return f"T{seconds}-M{giga}"


def print_version(binary: str = PRIMO_BINARY) -> int:
    """Answer `--version`: this wrapper's own version, the inner solver's
    `--version` output verbatim (or a `primo MISSING` placeholder if it could
    not be run), and a trailing `supports:` line.

    Matches ramparils-primo-cont/IDEAS.md item 1: `ramparils` is meant to call
    this once at init and log the whole block, so a wrapper/solver mismatch
    (`primo --version` prints `primo 0.1.0` for every build) is attributable
    from the log instead of only from `hostinfo.sh`/`versions.txt` after the
    fact. The placeholder line keeps the block's shape -- wrapper version,
    inner-solver line(s), `supports:` last -- the same whether or not the
    solver could be reached, so a probe can always read the last line; the
    return code is what actually signals failure.
    """
    print(f"{Path(sys.argv[0]).name} {WRAPPER_VERSION}")
    ok = True
    try:
        inner = subprocess.run(
            [binary, "--version"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"primo wrapper error: {error}", file=sys.stderr)
        print("primo MISSING")
        ok = False
    else:
        print(inner.stdout, end="")
        if inner.stdout and not inner.stdout.endswith("\n"):
            print()
    print(f"supports: {' '.join(SUPPORTS)}")
    return 0 if ok else 1


def print_params(arguments: list[str]) -> int:
    """Answer `--params`: print the primo command-line options *arguments*
    resolves to, and exit without running anything (IDEAS.md item 3).
    """
    try:
        strategy = build_strategy(parse_parameters(arguments))
    except ValueError as error:
        print(f"primo wrapper error: {error}", file=sys.stderr)
        return 1
    print(strategy)
    return 0


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "--version":
        return print_version()

    if len(sys.argv) >= 2 and sys.argv[1] == "--params":
        return print_params(sys.argv[2:])

    if len(sys.argv) < 3:
        print(
            f"usage: {Path(sys.argv[0]).name} INSTANCE CUTOFF [-name value ...]\n"
            f"       {Path(sys.argv[0]).name} --version\n"
            f"       {Path(sys.argv[0]).name} --params [-name value ...]",
            file=sys.stderr,
        )
        return 2

    instance = sys.argv[1]
    try:
        cutoff = float(sys.argv[2])
        if cutoff <= 0:
            raise ValueError("cutoff must be positive")
        parameters = parse_parameters(sys.argv[3:])
        strategy = build_strategy(parameters)
        solver = Primo(limit_string(cutoff, memory_limit_giga()))
        result = solver.solve(instance, strategy)
    except (KeyError, OSError, ValueError) as error:
        print(f"primo wrapper error: {error}", file=sys.stderr)
        status, runtime, quality, runhash = "UNKNOWN", locals().get("cutoff", 0.0), FAILURE_QUALITY, ""
    else:
        valid = solver.valid(result)
        if not valid:
            print(
                f"primo wrapper: invalid result for strategy {strategy!r}\n"
                f"{getattr(solver, '_output', '')}",
                file=sys.stderr,
            )
        # "UNKNOWN" is ramparils's own sentinel for "no parseable result
        # line" (eval.rs's parse_solver_output): reusing it here for a result
        # with no real SMT status (bad flag, crash, ...) routes it through
        # ramparils's existing machinery for free -- `log_crash` records the
        # exact command and full stdout/stderr for this instance, and the
        # result is skipped from the cache -- instead of being silently
        # accepted as a normal, cacheable outcome the way a made-up "error"
        # status did. A genuine solver status (unknown, timeout, ...) is
        # unaffected: `valid` is true, it keeps its real name, and stays
        # cached.
        status = result.get("status", "UNKNOWN") if valid else "UNKNOWN"
        if status in solver.success:
            runtime = result.get("runtime", cutoff)
            quality = 0.0
            # The runhash is only meaningful for a run that actually produced
            # `--stats` counters. A run with none of the PRIMO_RUNHASH_GEN
            # keys (crashed or killed before it could print any) hashes an
            # empty dict, which is a fixed constant, not a fingerprint of
            # what primo did -- comparing it across such runs would read as
            # "identical behaviour" for runs that share nothing but having no
            # data. sat/unsat runs always carry the full counter set, so
            # gating on success sidesteps that case entirely.
            runhash = f", {result['runhash']:016x}"
        else:
            # PAR1: charge the full cutoff, not whatever real time elapsed --
            # a run that fails or crashes quickly must not score better than
            # one that actually spent the budget solving something.
            runtime = cutoff
            quality = FAILURE_QUALITY
            runhash = ""

    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}{runhash}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
