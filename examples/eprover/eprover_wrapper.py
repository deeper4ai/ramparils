#!/usr/bin/env python3
"""RamParILS wrapper for the E prover, via solverpy's `E`.

Translates a RamParILS `-name value ...` parameter list into an E
command-line strategy and runs it through `solverpy.solver.atp.eprover.E`,
which supplies the time/memory limits (`timeout`/`ulimit -Sv`, both applied
to E's whole process group, so a RamParILS-level cancellation kills it too --
no signal handling needed here), SZS status parsing, and the `runhash`
fingerprint (see `RunHash`/`E_RUNHASH_GEN`) used to detect inert parameters.

Unlike `examples/primo/primo_wrapper.py`'s space (every primo QF_LRA option
this solver has), this is deliberately a *small* domain -- a handful of core
proof-search switches, term ordering, and up to 4 clause-selection heuristic
slots -- not the full combinatorial space `solverpy_grackle.trainer.eprover`
builds (in particular, each slot picks among 5 fixed named CEFs rather than
that module's full set of 20).
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from solverpy.solver.atp.eprover import E, E_BINARY

FAILURE_QUALITY = 10_000_000.0
DEFAULT_MEMORY_LIMIT_MB = 4096

WRAPPER_VERSION = "0.1.0"
# Features this wrapper answers `--version` with a `supports:` line for, per
# ramparils-primo-cont/IDEAS.md item 1.
SUPPORTS = ["version", "runhash", "params", "clean"]

# Boolean parameters, off by default: E is told about them only to turn them on.
POSITIVE_FLAGS = {
    "condense": "--condense",
    "presat": "--presat-simplify",
    "prefer": "--prefer-initial-clauses",
}

# Parameters passed through as `--flag=value`, always emitted when present --
# unlike primo's VALUE_OPTIONS, these have no "solver default" sentinel: E's
# own literal-selection/ordering defaults are not expressible as "omit the
# flag" the way a primo boolean's off-state is, so the strategy states them
# explicitly whenever RamParILS supplies a value, whether or not it happens to
# match E's own default.
VALUE_OPTIONS = {
    "sel": "--literal-selection-strategy",
    "tord": "--term-ordering",
    "tord_prec": "--order-precedence-generation",
    # Conditional on tord == KBO6; RamParILS omits it otherwise.
    "tord_weight": "--order-weight-generation",
}

# `defcnf`'s "none" is the sentinel: omit the flag, let E use its own
# default. "0" is a real, distinct value -- per `eprover --help`,
# `--definitional-cnf=0` actively disables definitional CNF, which is NOT
# the same as leaving the flag off. Every other value is emitted as-is.
DEFCNF_DEFAULT = "none"

# `fwdemod`'s "2" IS E's own default, so it is the sentinel here: present but
# equal to "2" emits nothing.
FWDEMOD_DEFAULT = "2"

# `tord_const`'s "0" means "unset"; "1" sets a constant weight of 1. Only
# meaningful (and only ever present) under tord == KBO6.
TORD_CONST_DEFAULT = "0"

# `simparamod`: three-valued, "none" needs no flag.
SIMPARAMOD_FLAGS = {
    "normal": "--simul-paramod",
    "oriented": "--oriented-simul-paramod",
}

# `der` (destructive equality resolution): five-valued, "none" needs no flag.
# Each later stage implies the earlier ones, matching
# `solverpy_grackle.trainer.eprover.runner.eprover.EproverRunner.args`.
DER_FLAGS = {
    "std": "--destructive-er",
    "strong": "--destructive-er --strong-destructive-er",
    "agg": "--destructive-er --destructive-er-aggressive",
    "stragg": "--destructive-er --destructive-er-aggressive --strong-destructive-er",
}

# Clause-selection heuristic: up to 4 slots, each an independent (freq, CEF)
# pair, composed into one `-H`/`--define-heuristic` definition -- the same
# shape as `solverpy_grackle.trainer.eprover.heuristic.HeuristicDomain`, but
# drawing from 5 fixed named CEFs (indices 0, 2, 5, 15, 19 of that module's
# `HEURISTIC_CEFS`, chosen to span distinct families) instead of its full set
# of 20. `slots` (0-4) picks how many are active; 0 means no `-H` flag at all
# -- E's own built-in default heuristic. `heurN`/`freqN` pick the CEF and its
# weight independently for slot N -- see `build_strategy` for how the active
# slots are assembled and `params-eprover-simple.txt` for the `slots in {...}`
# guards that make RamParILS supply exactly `slots` many (heurN, freqN) pairs.
HEURISTIC_CEFS = {
    "nb7": "ConjectureRelativeSymbolWeight(PreferGround,0.5,100,100,100,100,1.5,1.5,1)",
    "fifo": "FIFOWeight(PreferProcessed)",
    "precasc": "ConjectureRelativeSymbolWeight(SimulateSOS,0.5,100,100,100,100,1.5,1.5,1)",
    "mzr02": "Clauseweight(PreferWatchlist,20,9999,4)",
    "bls": "Clauseweight(PreferUnitGroundGoals,7,9999,5)",
}

MAX_SLOTS = 4

# Every RamParILS parameter's default, per params-eprover.txt -- the sentinel
# `clean` (see `clean_parameters`) strips a value against.
PARAM_DEFAULTS = {
    "sel": "SelectMaxLComplexAvoidPosPred",
    "simparamod": "none",
    "der": "none",
    "fwdemod": "2",
    "defcnf": "24",
    "condense": "0",
    "presat": "0",
    "prefer": "0",
    "tord": "LPO4",
    "tord_prec": "arity",
    "tord_weight": "arity",
    "tord_const": "0",
    "slots": "0",
    "heur1": "nb7",
    "freq1": "1",
    "heur2": "fifo",
    "freq2": "1",
    "heur3": "precasc",
    "freq3": "1",
    "heur4": "mzr02",
    "freq4": "1",
}

# The parameters that together define the `-H`/`--define-heuristic` clause --
# `slots` plus each active `heurN`/`freqN` pair, in the order `build_strategy`
# assembles them. `clean` prints this group last, after every other parameter
# sorted alphabetically, since splitting it by name (`freq1` before `heur1`)
# would obscure that they are one heuristic definition, not independent
# options.
HEURISTIC_PARAM_ORDER = [
    "slots",
    "heur1", "freq1",
    "heur2", "freq2",
    "heur3", "freq3",
    "heur4", "freq4",
]


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
    if normalized not in {"0", "1"}:
        raise ValueError(f"expected 0 or 1, got {value!r}")
    return normalized == "1"


def build_strategy(parameters: dict[str, str]) -> str:
    """Translate RamParILS parameters into an E strategy string.

    The strategy is the space-joined E command-line options, exactly what
    `E` appends after its static options (see
    `solverpy.solver.atp.eprover.E_STATIC`) and before the instance.

    Inactive conditional parameters (`tord_weight`, `tord_const`) are omitted
    from the command line by RamParILS, so every lookup here has to tolerate
    a missing name and fall back to E's own default.
    """
    options: list[str] = []

    for name, flag in POSITIVE_FLAGS.items():
        if name in parameters and is_true(parameters[name]):
            options.append(flag)

    for name, flag in VALUE_OPTIONS.items():
        if name in parameters:
            options.append(f"{flag}={parameters[name]}")

    if "defcnf" in parameters:
        value = parameters["defcnf"]
        if value != DEFCNF_DEFAULT:
            options.append(f"--definitional-cnf={value}")

    if "fwdemod" in parameters:
        value = parameters["fwdemod"]
        if value != FWDEMOD_DEFAULT:
            options.append(f"--forward-demod-level={value}")

    if "tord_const" in parameters:
        value = parameters["tord_const"]
        if value != TORD_CONST_DEFAULT:
            options.append(f"--order-constant-weight={value}")

    if "simparamod" in parameters:
        value = parameters["simparamod"]
        if value != "none":
            if value not in SIMPARAMOD_FLAGS:
                raise ValueError(f"unknown simparamod value: {value!r}")
            options.append(SIMPARAMOD_FLAGS[value])

    if "der" in parameters:
        value = parameters["der"]
        if value != "none":
            if value not in DER_FLAGS:
                raise ValueError(f"unknown der value: {value!r}")
            options.append(DER_FLAGS[value])

    if "slots" in parameters:
        n_slots = int(parameters["slots"])
        if not 0 <= n_slots <= MAX_SLOTS:
            raise ValueError(f"slots must be between 0 and {MAX_SLOTS}, got {n_slots}")
        if n_slots > 0:
            cefs: list[str] = []
            for i in range(1, n_slots + 1):
                heur_name, freq_name = f"heur{i}", f"freq{i}"
                # `heurN`/`freqN` are conditional on `slots in {i, i+1, ...}`
                # in the paramfile, so RamParILS always supplies exactly the
                # first `n_slots` pairs alongside a given `slots`; a caller
                # going around RamParILS (e.g. `--params`) that omits one is a
                # real space/wrapper mismatch, not something to paper over.
                if heur_name not in parameters or freq_name not in parameters:
                    raise ValueError(f"slots={n_slots} requires {heur_name} and {freq_name}")
                name = parameters[heur_name]
                if name not in HEURISTIC_CEFS:
                    raise ValueError(f"unknown heuristic in {heur_name}: {name!r}")
                cefs.append(f"{parameters[freq_name]}*{HEURISTIC_CEFS[name]}")
            cefs.append("1*FIFOWeight(ConstPrio)")
            cef = "(" + ",".join(cefs) + ")"
            options.append(f"--define-heuristic='{cef}'")

    return " ".join(options)


def memory_limit_giga() -> float:
    value = int(os.environ.get("EPROVER_MEMORY_MB", str(DEFAULT_MEMORY_LIMIT_MB)))
    if value <= 0:
        raise ValueError("EPROVER_MEMORY_MB must be positive")
    return value / 1024


def limit_string(cutoff: float, giga: float) -> str:
    """Build a solverpy limit string (`T<seconds>-M<gigabytes>`) from *cutoff*.

    RamParILS always calls the wrapper with a whole number of seconds (its
    fidelity schedule already rounds up via `.ceil()`), and `E`'s underlying
    `Limits` parser requires an integer `T`, so a fractional *cutoff* is
    rejected rather than silently truncated.
    """
    seconds = round(cutoff)
    if abs(cutoff - seconds) > 1e-6:
        raise ValueError(f"cutoff must be a whole number of seconds, got {cutoff!r}")
    return f"T{seconds}-M{giga}"


def print_version(binary: str = E_BINARY) -> int:
    """Answer `--version`: this wrapper's own version, the inner solver's
    `--version` output verbatim (or an `eprover MISSING` placeholder if it
    could not be run), and a trailing `supports:` line.

    Matches ramparils-primo-cont/IDEAS.md item 1: `ramparils` is meant to call
    this once at init and log the whole block. The placeholder line keeps the
    block's shape -- wrapper version, inner-solver line(s), `supports:` last
    -- the same whether or not the solver could be reached, so a probe can
    always read the last line; the return code is what actually signals
    failure.
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
        print(f"eprover wrapper error: {error}", file=sys.stderr)
        print("eprover MISSING")
        ok = False
    else:
        print(inner.stdout, end="")
        if inner.stdout and not inner.stdout.endswith("\n"):
            print()
    print(f"supports: {' '.join(SUPPORTS)}")
    return 0 if ok else 1


def print_params(arguments: list[str]) -> int:
    """Answer `--params`: print the E command-line options *arguments*
    resolves to, and exit without running anything (IDEAS.md item 3).
    """
    try:
        strategy = build_strategy(parse_parameters(arguments))
    except ValueError as error:
        print(f"eprover wrapper error: {error}", file=sys.stderr)
        return 1
    print(strategy)
    return 0


def is_active(name: str, parameters: dict[str, str]) -> bool:
    """Whether *name* is live under *parameters*, per params-eprover.txt's two
    guards: `tord_weight`/`tord_const` on `tord in {KBO6}`, `heurN`/`freqN` on
    `slots in {N, ..., MAX_SLOTS}`. Every other parameter is unconditional.
    """
    if name in ("tord_weight", "tord_const"):
        return parameters.get("tord") == "KBO6"
    if name.startswith("heur") or name.startswith("freq"):
        slot = int(name[len("heur"):] if name.startswith("heur") else name[len("freq"):])
        try:
            n_slots = int(parameters.get("slots", "0"))
        except ValueError:
            return False
        return n_slots >= slot
    return True


# `sel`/`tord` are never dropped on a default match, unlike the rest of
# PARAM_DEFAULTS: `eprover --help` states no actual default for
# `--literal-selection-strategy` or `--term-ordering` (unlike, say,
# `--forward-demod-level`), so params-eprover.txt's bracketed value is only
# this domain's default, not E's -- omitting the flag would leave E to pick
# its own, which need not agree.
ALWAYS_KEEP = {"sel", "tord"}


def clean_parameters(parameters: dict[str, str]) -> dict[str, str]:
    """Drop every parameter at its default value or under an inactive guard.

    `slots` and each active `heurN`/`freqN` are kept even at their default
    value: `build_strategy` requires exactly the first `n_slots` pairs to be
    present whenever `slots` is active, so dropping one on a default match
    would turn a valid heuristic definition into a missing-parameter error.
    `sel` and `tord` are kept regardless of value; see `ALWAYS_KEEP`.
    """
    cleaned: dict[str, str] = {}
    for name, value in parameters.items():
        if name not in PARAM_DEFAULTS:
            raise ValueError(f"unknown parameter: {name!r}")
        if not is_active(name, parameters):
            continue
        if (
            name not in HEURISTIC_PARAM_ORDER
            and name not in ALWAYS_KEEP
            and value == PARAM_DEFAULTS[name]
        ):
            continue
        cleaned[name] = value
    return cleaned


def print_clean(arguments: list[str]) -> int:
    """Answer `--clean`: like `--params`, but print the `-name value` pairs
    themselves (not the E options they resolve to) with defaulted and
    inactive-guard parameters dropped, sorted by name -- except `slots` and
    the `heurN`/`freqN` pairs it activates, which print last as one group in
    build order rather than being scattered alphabetically (`freq1` before
    `heur1`) across the rest.
    """
    try:
        cleaned = clean_parameters(parse_parameters(arguments))
    except ValueError as error:
        print(f"eprover wrapper error: {error}", file=sys.stderr)
        return 1
    ordered = sorted(name for name in cleaned if name not in HEURISTIC_PARAM_ORDER)
    ordered += [name for name in HEURISTIC_PARAM_ORDER if name in cleaned]
    parts = [token for name in ordered for token in (f"-{name}", cleaned[name])]
    print(" ".join(parts))
    return 0


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "--version":
        return print_version()

    if len(sys.argv) >= 2 and sys.argv[1] == "--params":
        return print_params(sys.argv[2:])

    if len(sys.argv) >= 2 and sys.argv[1] == "--clean":
        return print_clean(sys.argv[2:])

    if len(sys.argv) < 3:
        print(
            f"usage: {Path(sys.argv[0]).name} INSTANCE CUTOFF [-name value ...]\n"
            f"       {Path(sys.argv[0]).name} --version\n"
            f"       {Path(sys.argv[0]).name} --params [-name value ...]\n"
            f"       {Path(sys.argv[0]).name} --clean [-name value ...]",
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
        solver = E(limit_string(cutoff, memory_limit_giga()))
        result = solver.solve(instance, strategy)
    except (KeyError, OSError, ValueError) as error:
        print(f"eprover wrapper error: {error}", file=sys.stderr)
        status, runtime, quality, runhash = "UNKNOWN", locals().get("cutoff", 0.0), FAILURE_QUALITY, ""
    else:
        valid = solver.valid(result)
        if not valid:
            print(
                f"eprover wrapper: invalid result for strategy {strategy!r}\n"
                f"{getattr(solver, '_output', '')}",
                file=sys.stderr,
            )
        # "UNKNOWN" is ramparils's own sentinel for "no parseable result
        # line" (eval.rs's parse_solver_output): reusing it here for a result
        # with no real SZS status (bad flag, crash, ...) routes it through
        # ramparils's existing machinery for free -- `log_crash` records the
        # exact command and full stdout/stderr for this instance, and the
        # result is skipped from the cache -- instead of being silently
        # accepted as a normal, cacheable outcome the way a made-up "ERROR"
        # status did. A genuine solver status (ResourceOut, GaveUp, ...) is
        # unaffected: `valid` is true, it keeps its real name, and stays
        # cached.
        status = result.get("status", "UNKNOWN") if valid else "UNKNOWN"
        if status in solver.success:
            runtime = result.get("runtime", cutoff)
            quality = 0.0
            # Only meaningful for a run that actually produced statistics; see
            # the same reasoning in primo_wrapper.py -- an ERROR/timeout run
            # hashes an empty selection, a fixed constant carrying no
            # information, so gating on success sidesteps it entirely.
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
