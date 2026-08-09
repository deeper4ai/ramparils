#!/usr/bin/env python3
"""Direct RamParILS wrapper for the primo QF_LRA solver."""

from __future__ import annotations

from functools import partial
import os
import resource
import signal
import subprocess
import sys
import time
from pathlib import Path


FAILURE_QUALITY = 10_000_000.0
DEFAULT_MEMORY_LIMIT_MB = 4096
solver_process: subprocess.Popen[str] | None = None

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


def build_command(
    executable: str, instance: str, parameters: dict[str, str]
) -> list[str]:
    # Inactive conditional parameters are omitted from the command line by
    # RamParILS, so every lookup here has to tolerate a missing name and fall
    # back to primo's own default.
    command = [executable, "--quiet"]

    for name, flag in NEGATIVE_FLAGS.items():
        if name in parameters and not is_true(parameters[name]):
            command.append(flag)

    for name, flag in POSITIVE_FLAGS.items():
        if name in parameters and is_true(parameters[name]):
            command.append(flag)

    for name, flag in VALUE_OPTIONS.items():
        if name in parameters:
            command.extend([flag, parameters[name]])

    if "top_level_or_tseitin" in parameters:
        mode = parameters["top_level_or_tseitin"]
        if mode not in TSEITIN_FLAGS:
            raise ValueError(f"unknown top_level_or_tseitin mode: {mode!r}")
        flag = TSEITIN_FLAGS[mode]
        if flag is not None:
            command.append(flag)

    command.append(instance)
    return command


def solver_status(stdout: str) -> str | None:
    for line in stdout.splitlines():
        status = line.strip().lower()
        if status in {"sat", "unsat", "unknown"}:
            return status
    return None


def memory_limit_bytes() -> int:
    value = int(os.environ.get("PRIMO_MEMORY_MB", str(DEFAULT_MEMORY_LIMIT_MB)))
    if value <= 0:
        raise ValueError("PRIMO_MEMORY_MB must be positive")
    return value * 1024 * 1024


def apply_process_setup(limit: int, signal_mask: set[signal.Signals]) -> None:
    resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
    signal.pthread_sigmask(signal.SIG_SETMASK, signal_mask)


def terminate_solver() -> None:
    global solver_process
    if solver_process is None or solver_process.poll() is not None:
        return
    try:
        os.killpg(solver_process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def handle_termination(signum: int, _frame: object) -> None:
    terminate_solver()
    raise SystemExit(128 + signum)


def run(
    command: list[str], cutoff: float, memory_limit: int
) -> tuple[str, float, float]:
    global solver_process
    started = time.monotonic()
    termination_signals = {signal.SIGINT, signal.SIGTERM}
    previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, termination_signals)
    try:
        solver_process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            preexec_fn=partial(apply_process_setup, memory_limit, previous_mask),
        )
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)

    try:
        stdout, stderr = solver_process.communicate(timeout=cutoff)
    except subprocess.TimeoutExpired:
        terminate_solver()
        solver_process.communicate()
        return "timeout", cutoff, FAILURE_QUALITY
    finally:
        process = solver_process
        solver_process = None

    elapsed = min(time.monotonic() - started, cutoff)
    status = solver_status(stdout)
    if process.returncode == 0 and status in {"sat", "unsat"}:
        return status, elapsed, 0.0

    if stderr:
        print(stderr, file=sys.stderr, end="")
    return status or "error", cutoff, FAILURE_QUALITY


def main() -> int:
    signal.signal(signal.SIGINT, handle_termination)
    signal.signal(signal.SIGTERM, handle_termination)

    if len(sys.argv) < 3:
        print(
            f"usage: {Path(sys.argv[0]).name} INSTANCE CUTOFF [-name value ...]",
            file=sys.stderr,
        )
        return 2

    instance = sys.argv[1]
    try:
        cutoff = float(sys.argv[2])
        if cutoff <= 0:
            raise ValueError("cutoff must be positive")
        parameters = parse_parameters(sys.argv[3:])
        executable = os.environ.get("PRIMO", "primo")
        command = build_command(executable, instance, parameters)
        status, runtime, quality = run(command, cutoff, memory_limit_bytes())
    except (KeyError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"primo wrapper error: {error}", file=sys.stderr)
        status, runtime, quality = (
            "error",
            locals().get("cutoff", 0.0),
            FAILURE_QUALITY,
        )

    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
