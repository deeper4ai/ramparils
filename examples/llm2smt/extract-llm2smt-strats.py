#!/usr/bin/env python3
"""Reconstruct native llm2smt strategies from a RamParILS debug log.

Default support files are resolved relative to this script.
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import shlex
from pathlib import Path
from types import ModuleType


INITIAL_MARKER = "ils: initial config:"
TIMESTAMP_LINE = re.compile(r"^\[\s*\d")
CONFIG_LINE = re.compile(r"^\s+([A-Za-z0-9_]+):\s+(.+?)\s*$")
INCUMBENT_LINE = re.compile(
    r"ils: new incumbent: hash=([0-9a-fA-F]+)\b"
)
CHANGE_LINE = re.compile(
    r"^\s+([A-Za-z0-9_]+):\s+(.+?)\s+->\s+(.+?)\s*$"
)
PARAM_LINE = re.compile(
    r"^([A-Za-z0-9_]+)\s+\{[^}]*\}\s+\[([^]]+)\]"
    r"(?:\s*\|\s*([A-Za-z0-9_]+)\s+in\s+\{([^}]*)\})?$"
)


def parse_paramfile(
    path: Path,
) -> tuple[dict[str, str], dict[str, tuple[str, set[str]]]]:
    defaults: dict[str, str] = {}
    conditions: dict[str, tuple[str, set[str]]] = {}

    for raw_line in path.read_text().splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        match = PARAM_LINE.match(line)
        if match is None:
            continue
        name, default, parent, values = match.groups()
        defaults[name] = default.strip()
        if parent is not None and values is not None:
            conditions[name] = (
                parent,
                {value.strip() for value in values.split(",")},
            )

    if not defaults:
        raise ValueError(f"no parameter defaults found in {path}")
    return defaults, conditions


def active_config(
    config: dict[str, str],
    conditions: dict[str, tuple[str, set[str]]],
) -> dict[str, str]:
    active = set(config)
    changed = True
    while changed:
        changed = False
        for name, (parent, allowed) in conditions.items():
            if name in active and (
                parent not in active or config.get(parent) not in allowed
            ):
                active.remove(name)
                changed = True
    return {name: value for name, value in config.items() if name in active}


def load_wrapper(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("llm2smt_wrapper", path)
    if spec is None or spec.loader is None:
        raise ValueError(f"cannot load wrapper module from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if not callable(getattr(module, "build_command", None)):
        raise ValueError(f"{path} does not define build_command()")
    return module


def parse_initial_config(lines: list[str]) -> tuple[dict[str, str], int]:
    for index, line in enumerate(lines):
        if INITIAL_MARKER not in line:
            continue

        config: dict[str, str] = {}
        cursor = index + 1
        while cursor < len(lines) and not TIMESTAMP_LINE.match(lines[cursor]):
            match = CONFIG_LINE.match(lines[cursor])
            if match:
                config[match.group(1)] = match.group(2)
            cursor += 1

        if not config:
            raise ValueError("initial configuration block is empty")
        return config, cursor

    raise ValueError("log does not contain an initial configuration")


def parse_incumbents(
    lines: list[str], start: int, initial: dict[str, str]
) -> list[tuple[str, dict[str, str]]]:
    config = initial.copy()
    incumbents: list[tuple[str, dict[str, str]]] = []
    index = start

    while index < len(lines):
        incumbent = INCUMBENT_LINE.search(lines[index])
        if incumbent is None:
            index += 1
            continue

        hash_value = incumbent.group(1).lower()
        index += 1
        changes = 0

        while index < len(lines) and not TIMESTAMP_LINE.match(lines[index]):
            change = CHANGE_LINE.match(lines[index])
            if change:
                name, old_value, new_value = change.groups()
                actual = config.get(name)
                if actual is None:
                    raise ValueError(
                        f"incumbent {hash_value}: unknown parameter {name!r}"
                    )
                if actual != old_value:
                    raise ValueError(
                        f"incumbent {hash_value}: {name} was {actual!r}, "
                        f"log expected {old_value!r}"
                    )
                config[name] = new_value
                changes += 1
            index += 1

        if changes == 0:
            raise ValueError(
                f"incumbent {hash_value}: no configuration changes found"
            )
        incumbents.append((hash_value, config.copy()))

    if not incumbents:
        raise ValueError("log does not contain any incumbent records")
    return incumbents


def native_arguments(wrapper: ModuleType, config: dict[str, str]) -> list[str]:
    command = wrapper.build_command("llm2smt", "INSTANCE", config)
    if (
        len(command) < 3
        or command[0] != "llm2smt"
        or command[1] != "--quiet"
        or command[-1] != "INSTANCE"
    ):
        raise ValueError("wrapper build_command() returned an unexpected command")
    return command[2:-1]


def write_strategies(
    initial: dict[str, str],
    incumbents: list[tuple[str, dict[str, str]]],
    output_dir: Path,
    wrapper: ModuleType,
    conditions: dict[str, tuple[str, set[str]]],
    prefix: str,
) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    initial_active = active_config(initial, conditions)
    initial_path = output_dir / f"{prefix}-init"
    initial_path.write_text(
        shlex.join(native_arguments(wrapper, initial_active)) + "\n"
    )
    print(initial_path)

    for hash_value, config in incumbents:
        active = active_config(config, conditions)
        arguments = native_arguments(wrapper, active)
        path = output_dir / f"{prefix}-{hash_value}"
        path.write_text(shlex.join(arguments) + "\n")
        print(path)


def main() -> int:
    example_dir = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(
        description=(
            "Reconstruct every incumbent in a RamParILS debug log and "
            "write native llm2smt command-line arguments."
        )
    )
    parser.add_argument("log", type=Path, help="RamParILS debug log")
    parser.add_argument(
        "-o",
        "--output-dir",
        type=Path,
        help="destination directory (default: LOG_DIR/strats)",
    )
    parser.add_argument(
        "--prefix",
        default="ram",
        help="strategy filename prefix (default: ram)",
    )
    parser.add_argument(
        "--paramfile",
        type=Path,
        default=example_dir / "params-llm2smt.txt",
        help="parameter file used to recover inactive defaults",
    )
    parser.add_argument(
        "--wrapper",
        type=Path,
        default=example_dir / "llm2smt_wrapper.py",
        help="llm2smt wrapper containing build_command()",
    )
    args = parser.parse_args()

    log = args.log.resolve()
    output_dir = (
        args.output_dir.resolve()
        if args.output_dir is not None
        else log.parent / "strats"
    )
    prefix = args.prefix.strip()
    if not prefix or prefix in {".", ".."} or "/" in prefix:
        parser.error("prefix must be a non-empty filename component")

    try:
        lines = log.read_text().splitlines()
        defaults, conditions = parse_paramfile(args.paramfile.resolve())
        logged_initial, start = parse_initial_config(lines)
        initial = defaults.copy()
        initial.update(logged_initial)
        incumbents = parse_incumbents(lines, start, initial)
        wrapper = load_wrapper(args.wrapper.resolve())
        write_strategies(
            initial, incumbents, output_dir, wrapper, conditions, prefix
        )
    except (OSError, ValueError) as error:
        parser.error(str(error))

    print(
        f"Wrote the initial strategy and {len(incumbents)} incumbents "
        f"to {output_dir}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
