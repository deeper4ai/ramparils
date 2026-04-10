"""
RamParILS — parallel automated algorithm configuration via Iterated Local Search.

The public API is a single function: :func:`specialize`.
"""

from typing import Optional
from ._ramparils import specialize as _specialize


def specialize(
    strategy: dict[str, str],
    scenario: dict,
    cache_db: str,
    cores: int = 0,
) -> dict[str, str]:
    """Specialize a strategy on a set of benchmark instances using FocusedILS.

    Runs Iterated Local Search starting from *strategy*, evaluating
    ``(configuration, instance)`` pairs in parallel, and returns the best
    configuration found within the time budget.

    Results are cached in a persistent SQLite database so that repeated calls
    with the same algorithm and instances never redo solver invocations.

    Args:
        strategy: Initial parameter configuration as ``{name: value}`` strings.
            Must contain every parameter defined in ``scenario["paramfile"]``.
            Values are strings even for numeric parameters (e.g. ``"1.189"``).
        scenario: Tuning scenario as a dict.  Required keys:

            - **algo** (*str*) — command to invoke the target algorithm.
              Invoked as ``<algo> <instance> <cutoff_time> -p1 v1 -p2 v2 …``
            - **paramfile** (*str*) — path to the ``.params`` file describing
              the parameter space (domains, defaults, conditionals, forbidden).
            - **cutoff_time** (*float*) — per-run time limit in seconds,
              passed to the target algorithm.
            - **tuner_timeout** (*float*) — total wall-clock budget for the
              tuner in seconds.

            Exactly one of the following must be supplied to specify instances:

            - **instances** (*list[str]*) — list of instance paths directly.
            - **instance_file** (*str*) — path to a text file with one instance
              path per line (blank lines and ``#`` comments are ignored).

            Optional keys:

            - **run_obj** (*str*, default ``"runtime"``) — what a single run
              optimises: ``"runtime"`` or ``"quality"``.
            - **overall_obj** (*str*, default ``"mean"``) — how per-run results
              are aggregated: ``"mean"`` or ``"median"``.
            - **test_instance_file** (*str*) — reserved for future use.

        cache_db: Path to the SQLite cache file.  Created if it does not exist.
            Use ``":memory:"`` for an in-process cache that is not persisted.
        cores: Number of parallel worker threads.  ``0`` (default) uses all
            available CPU cores.

    Returns:
        The best configuration found, as ``{name: value}`` strings.
        Only *active* parameters are included (inactive conditional parameters
        are omitted).

    Raises:
        RuntimeError: If the scenario is invalid, the instance list is empty,
            the paramfile cannot be parsed, or the cache cannot be opened.

    Example:
        >>> result = ramparils.specialize(
        ...     strategy={"alpha": "1.189", "rho": "0.5"},
        ...     scenario={
        ...         "algo":          "ruby /path/to/solver_wrapper.rb",
        ...         "paramfile":     "/path/to/solver.params",
        ...         "instances":     ["/path/to/inst1.cnf", "/path/to/inst2.cnf"],
        ...         "cutoff_time":   5.0,
        ...         "tuner_timeout": 120.0,
        ...     },
        ...     cache_db="/tmp/ramparils_cache.db",
        ...     cores=8,
        ... )
        >>> print(result)
        {'alpha': '1.256', 'rho': '0.5'}
    """
    return _specialize(strategy=strategy, scenario=scenario, cache_db=cache_db, cores=cores)


__all__ = ["specialize"]
