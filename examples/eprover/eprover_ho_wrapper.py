#!/usr/bin/env python3
"""RamParILS wrapper for the E prover's HO domain, via solverpy's `E`.

Sibling of `eprover_wrapper.py`, for `params-eprover-ho.txt` instead of
`params-eprover.txt`. Same translation job -- RamParILS `-name value ...`
parameters into an E command line, run through `solverpy.solver.atp.eprover.E`
-- but over the superset domain: everything `eprover_wrapper.py` supports,
plus HO extension rules and lambda/injectivity handling, more
preprocessing/SAT-checking switches, and 5 heuristic slots (was 4) drawing
from 20 named CEFs (was 5). See `params-eprover-ho.txt`'s header comment for
what is genuinely new versus carried over unchanged, and its E-source
citations for where each flag is verified
(`/home/yan/repos/cbboyan/eprover`, checked out locally).

`--delete-bad-limit` is fixed at 2000000000 (e-nb7's own value) rather than
exposed as a parameter -- see params-eprover-ho.txt's header for why.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from solverpy.solver.atp.eprover import E

FAILURE_QUALITY = 10_000_000.0
DEFAULT_MEMORY_LIMIT_MB = 4096

# This domain's whole point is the HO extension rules/lambda handling
# (neg_ext, pos_ext, ho_order_kind, ...), which plain "eprover" rejects
# outright ("To support HOL reasoning, recompile E using './configure
# --enable-ho && make rebuild'") -- unlike eprover_wrapper.py, which is fine
# with solverpy's own "eprover" default since its domain has no HO options at
# all. Override with EPROVER_BINARY if a different HO build is on PATH.
DEFAULT_BINARY = "eprover-ho"

WRAPPER_VERSION = "0.1.0"
SUPPORTS = ["version", "runhash", "params", "clean"]

# `e-nb7`'s own value; see params-eprover-ho.txt's header for why this is
# fixed rather than tunable, and why this constant over E's own OptArg default
# (1500000) or solverpy_grackle's runner constant (150000000).
DELETE_BAD_LIMIT = 2_000_000_000

# Boolean parameters domained {0,1}, off by default: E is told about them
# only to turn them on. Carried over from eprover_wrapper.py plus the
# preprocessing switches params-eprover-ho.txt adds (all NoArg flags in E's
# own PROVER/e_options.h).
POSITIVE_FLAGS = {
    "condense": "--condense",
    "presat": "--presat-simplify",
    "prefer": "--prefer-initial-clauses",
    "forwardcntxtsr": "--forward-context-sr",
    "splaggr": "--split-aggressive",
    "srd": "--split-reuse-defs",
    "strong_rw_inst": "--strong-rw-inst",
    "no_eq_unfolding": "--no-eq-unfolding",
    "sos_input_types": "--sos-uses-input-types",
}

# Boolean parameters domained {false,true} (not {0,1}) with "false" as E's own
# default -- also NoArg flags, but params-eprover-ho.txt spells their values
# as true/false to match the ho.py convention (lift_lambdas, local_rw,
# fool_unroll) they sit beside, not because E itself takes an argument here.
POSITIVE_FLAGS_TF = {
    "inverse_recognition": "--inverse-recognition",
    "replace_inj_defs": "--replace-inj-defs",
}

# Parameters passed through as `--flag=value`, always emitted when present --
# these have no "solver default" sentinel, matching eprover_wrapper.py's own
# VALUE_OPTIONS convention (and its comment on why: sel/tord/tord_prec have no
# expressible "omit the flag" state in E's own defaults).
VALUE_OPTIONS = {
    "sel": "--literal-selection-strategy",
    "tord": "--term-ordering",
    "tord_prec": "--order-precedence-generation",
    "tord_weight": "--order-weight-generation",
}

DEFCNF_DEFAULT = "none"
FWDEMOD_DEFAULT = "2"
TORD_CONST_DEFAULT = "0"
SPLCL_DEFAULT = "0"

SIMPARAMOD_FLAGS = {
    "normal": "--simul-paramod",
    "oriented": "--oriented-simul-paramod",
}

DER_FLAGS = {
    "std": "--destructive-er",
    "strong": "--destructive-er --strong-destructive-er",
    "agg": "--destructive-er --destructive-er-aggressive",
    "stragg": "--destructive-er --destructive-er-aggressive --strong-destructive-er",
}

# --neg-ext/--pos-ext (PROVER/eprover.c:2010-2045): "off" is E's own default
# (no flag), "max"/"all" are ReqArg values. "max" is not offered by
# solverpy_grackle's HoDomain; added here since the E source accepts it.
EXT_FLAGS = {
    "neg_ext": "--neg-ext",
    "pos_ext": "--pos-ext",
}
EXT_DEFAULT = "off"

EXT_SUP_MAX_DEPTH_DEFAULT = "-1"
LIFT_LAMBDAS_DEFAULT = "true"
LOCAL_RW_DEFAULT = "false"
FOOL_UNROLL_DEFAULT = "true"
HO_ORDER_KIND_DEFAULT = "lfho"

# `satcheck`'s "none" is this wrapper's own sentinel (omit the flag, E's own
# default) -- deliberately distinct from CLAUSES/ccl_satinterface.c's
# "NoGrounding" name, which params-eprover-ho.txt leaves out of the domain for
# exactly that reason (two spellings of the same "off" state). E always pairs
# an active satcheck with --satcheck-proc-interval=5000, matching
# solverpy_grackle's runner (args() in runner/eprover.py) rather than exposing
# the interval as a further tunable.
SATCHECK_DEFAULT = "none"
SATCHECK_PROC_INTERVAL = 5000

# Clause-selection heuristic: up to 5 slots, each an independent (heurN,
# freqN) pair -- freq tuned separately from the CEF it multiplies. Reduced
# 2026-09-02 (user) from all 20 named CEFs in
# solverpy_grackle.trainer.eprover.heuristic.HEURISTIC_CEFS back down to just
# the 5 that e-nb7 itself uses (indices 0-4, its "nb7/new_bool family") --
# the wider 20-CEF/6-freq space made the domain enormous (1.7e23
# configurations) without buying anything for a run seeded at e-nb7, since
# nothing steers the search toward the other 15 over that first pass. The
# other 15 names/CEFs and the fuller freq domain are recoverable from git
# history (see the commit that reduced this) if a later run wants them back.
HEURISTIC_CEFS = {
    "nb7": "ConjectureRelativeSymbolWeight(PreferGround,0.5,100,100,100,100,1.5,1.5,1)",  # 0
    "nb7dd": "ConjectureRelativeSymbolWeight(ByDerivationDepth,0.1,100,100,100,100,1.5,1.5,1.5)",  # 1
    "fifo": "FIFOWeight(PreferProcessed)",  # 2
    "nb7ng": "ConjectureRelativeSymbolWeight(PreferNonGoals,0.5,100,100,100,100,1.5,1.5,1)",  # 3
    "refgoals": "Refinedweight(PreferGoals,3,2,2,1.5,2)",  # 4
}

MAX_SLOTS = 5

PARAM_DEFAULTS = {
    "sel": "SelectMaxLComplexAvoidPosPred",
    "simparamod": "none",
    "der": "none",
    "forwardcntxtsr": "0",
    "fwdemod": "2",
    "defcnf": "24",
    "condense": "0",
    "presat": "0",
    "prefer": "0",
    "splaggr": "0",
    "srd": "0",
    "splcl": "0",
    "strong_rw_inst": "0",
    "no_eq_unfolding": "0",
    "sos_input_types": "0",
    "satcheck": "none",
    "neg_ext": "off",
    "pos_ext": "off",
    "ext_sup_max_depth": "-1",
    "lift_lambdas": "true",
    "local_rw": "false",
    "fool_unroll": "true",
    "inverse_recognition": "false",
    "replace_inj_defs": "false",
    "ho_order_kind": "lfho",
    "tord": "LPO4",
    "tord_prec": "arity",
    "tord_weight": "arity",
    "tord_const": "0",
    "slots": "0",
    "heur1": "nb7",
    "freq1": "1",
    "heur2": "nb7dd",
    "freq2": "1",
    "heur3": "fifo",
    "freq3": "1",
    "heur4": "nb7ng",
    "freq4": "1",
    "heur5": "refgoals",
    "freq5": "1",
}

HEURISTIC_PARAM_ORDER = [
    "slots",
    "heur1", "freq1",
    "heur2", "freq2",
    "heur3", "freq3",
    "heur4", "freq4",
    "heur5", "freq5",
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

    Inactive conditional parameters (`tord_weight`, `tord_const`, and any
    `heurN`/`freqN` past `slots`) are omitted from the command line by
    RamParILS, so every lookup here tolerates a missing name and falls back to
    E's own default.
    """
    options: list[str] = [f"--delete-bad-limit={DELETE_BAD_LIMIT}"]

    for name, flag in POSITIVE_FLAGS.items():
        if name in parameters and is_true(parameters[name]):
            options.append(flag)

    for name, flag in POSITIVE_FLAGS_TF.items():
        if name in parameters and parameters[name] == "true":
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

    if "splcl" in parameters:
        value = parameters["splcl"]
        if value != SPLCL_DEFAULT:
            options.append(f"--split-clauses={value}")

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

    for name, flag in EXT_FLAGS.items():
        if name in parameters:
            value = parameters[name]
            if value != EXT_DEFAULT:
                options.append(f"{flag}={value}")

    if "ext_sup_max_depth" in parameters:
        value = parameters["ext_sup_max_depth"]
        if value != EXT_SUP_MAX_DEPTH_DEFAULT:
            options.append(f"--ext-sup-max-depth={value}")

    if "lift_lambdas" in parameters:
        value = parameters["lift_lambdas"]
        if value != LIFT_LAMBDAS_DEFAULT:
            options.append(f"--lift-lambdas={value}")

    if "local_rw" in parameters:
        value = parameters["local_rw"]
        if value != LOCAL_RW_DEFAULT:
            options.append(f"--local-rw={value}")

    if "fool_unroll" in parameters:
        value = parameters["fool_unroll"]
        if value != FOOL_UNROLL_DEFAULT:
            options.append(f"--fool-unroll={value}")

    if "ho_order_kind" in parameters:
        value = parameters["ho_order_kind"]
        if value != HO_ORDER_KIND_DEFAULT:
            options.append(f"--ho-order-kind={value}")

    if "satcheck" in parameters:
        value = parameters["satcheck"]
        if value != SATCHECK_DEFAULT:
            options.append(f"--satcheck={value} --satcheck-proc-interval={SATCHECK_PROC_INTERVAL}")

    if "slots" in parameters:
        n_slots = int(parameters["slots"])
        if not 0 <= n_slots <= MAX_SLOTS:
            raise ValueError(f"slots must be between 0 and {MAX_SLOTS}, got {n_slots}")
        if n_slots > 0:
            cefs: list[str] = []
            for i in range(1, n_slots + 1):
                heur_name, freq_name = f"heur{i}", f"freq{i}"
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
    seconds = round(cutoff)
    if abs(cutoff - seconds) > 1e-6:
        raise ValueError(f"cutoff must be a whole number of seconds, got {cutoff!r}")
    return f"T{seconds}-M{giga}"


def binary_name() -> str:
    return os.environ.get("EPROVER_BINARY", DEFAULT_BINARY)


def print_version(binary: str = "") -> int:
    binary = binary or binary_name()
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
    try:
        strategy = build_strategy(parse_parameters(arguments))
    except ValueError as error:
        print(f"eprover wrapper error: {error}", file=sys.stderr)
        return 1
    print(strategy)
    return 0


def is_active(name: str, parameters: dict[str, str]) -> bool:
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


ALWAYS_KEEP = {"sel", "tord"}


def clean_parameters(parameters: dict[str, str]) -> dict[str, str]:
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
        solver = E(limit_string(cutoff, memory_limit_giga()), binary=binary_name())
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
        status = result.get("status", "UNKNOWN") if valid else "UNKNOWN"
        if status in solver.success:
            runtime = result.get("runtime", cutoff)
            quality = 0.0
            runhash = f", {result['runhash']:016x}"
        else:
            runtime = cutoff
            quality = FAILURE_QUALITY
            runhash = ""

    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}{runhash}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
