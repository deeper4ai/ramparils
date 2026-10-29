#!/usr/bin/env python3
"""Direct RamParILS wrapper for the llm2smt QF_EUF solver."""

from __future__ import annotations

from functools import partial
import os
import resource
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path


FAILURE_QUALITY = 10_000_000.0
WRAPPER_VERSION = "0.2.0"
SUPPORTS = ["version"]
DEFAULT_MEMORY_LIMIT_MB = 4096
solver_process: subprocess.Popen[str] | None = None


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


def memory_limit_bytes() -> int:
    value = int(
        os.environ.get("LLM2SMT_MEMORY_MB", str(DEFAULT_MEMORY_LIMIT_MB))
    )
    if value <= 0:
        raise ValueError("LLM2SMT_MEMORY_MB must be positive")
    return value * 1024 * 1024


def apply_process_setup(
    limit: int, signal_mask: set[signal.Signals]
) -> None:
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
    previous_mask = signal.pthread_sigmask(
        signal.SIG_BLOCK, termination_signals
    )
    try:
        solver_process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            preexec_fn=partial(
                apply_process_setup, memory_limit, previous_mask
            ),
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
    # No status line at all means the wrapper got no answer: UNKNOWN (logged,
    # never cached). A real non-success status keeps its name.
    return status or "UNKNOWN", cutoff, FAILURE_QUALITY


def executable_name() -> str:
    return os.environ.get("LLM2SMT", "llm2smt")


def print_version() -> int:
    """Answer RamParILS's startup probe; exit 1 when llm2smt is unreachable."""
    executable = executable_name()
    print(f"{Path(sys.argv[0]).name} {WRAPPER_VERSION}")
    ok = shutil.which(executable) is not None
    if not ok:
        print(f"llm2smt wrapper error: {executable!r} not found", file=sys.stderr)
        print("llm2smt MISSING")
    else:
        try:
            inner = subprocess.run(
                [executable, "--version"], capture_output=True, text=True, timeout=30
            )
            text = inner.stdout.strip() if inner.returncode == 0 else ""
        except (OSError, subprocess.SubprocessError):
            text = ""
        print(text or f"llm2smt ({shutil.which(executable)}, version not reported)")
    print(f"supports: {' '.join(SUPPORTS)}")
    return 0 if ok else 1


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "--version":
        return print_version()

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
        executable = executable_name()
        command = build_command(executable, instance, parameters)
        status, runtime, quality = run(
            command, cutoff, memory_limit_bytes()
        )
    except (KeyError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"llm2smt wrapper error: {error}", file=sys.stderr)
        status, runtime, quality = (
            "UNKNOWN",
            locals().get("cutoff", 0.0),
            FAILURE_QUALITY,
        )

    print(f"#%# RamParIls #%# {status}, {runtime:.6f}, {quality:.1f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
