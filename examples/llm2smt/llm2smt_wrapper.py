#!/usr/bin/env python3
"""Direct RamParILS wrapper for the llm2smt QF_EUF solver."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from pathlib import Path


FAILURE_QUALITY = 10_000_000.0


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
    command = [
        executable,
        "--quiet",
        "--preprocess-passes",
        parameters["preprocess_passes"],
    ]

    positive_flags = {
        "nnf": "--nnf",
        "nnf_memo": "--nnf-memo",
        "eq_bridge": "--eq-bridge",
    }
    for name, flag in positive_flags.items():
        if name in parameters and is_true(parameters[name]):
            command.append(flag)

    negative_flags = {
        "nary": "--no-nary",
        "flatten": "--no-flatten",
        "finite_domain_amo": "--no-finite-domain-amo",
        "finite_domain_eq_defs": "--no-finite-domain-eqdefs",
        "theory_prop": "--no-theory-prop",
    }
    for name, flag in negative_flags.items():
        if not is_true(parameters[name]):
            command.append(flag)

    command.extend(
        [
            "--prop-interval",
            parameters["prop_interval"],
            "--prop-assign-threshold",
            parameters["prop_assign_threshold"],
            "--prop-delivery-budget",
            parameters["prop_delivery_budget"],
            instance,
        ]
    )
    return command


def solver_status(stdout: str) -> str | None:
    for line in stdout.splitlines():
        status = line.strip().lower()
        if status in {"sat", "unsat", "unknown"}:
            return status
    return None


def run(command: list[str], cutoff: float) -> tuple[str, float, float]:
    started = time.monotonic()
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=cutoff)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        return "timeout", cutoff, FAILURE_QUALITY

    elapsed = min(time.monotonic() - started, cutoff)
    status = solver_status(stdout)
    if process.returncode == 0 and status in {"sat", "unsat"}:
        return status, elapsed, 0.0

    if stderr:
        print(stderr, file=sys.stderr, end="")
    return status or "error", cutoff, FAILURE_QUALITY


def main() -> int:
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
        executable = os.environ.get("LLM2SMT", "llm2smt")
        command = build_command(executable, instance, parameters)
        status, runtime, quality = run(command, cutoff)
    except (KeyError, OSError, ValueError) as error:
        print(f"llm2smt wrapper error: {error}", file=sys.stderr)
        status, runtime, quality = (
            "error",
            locals().get("cutoff", 0.0),
            FAILURE_QUALITY,
        )

    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
