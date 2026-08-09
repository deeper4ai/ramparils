"""
RamParILS — parallel automated algorithm configuration via Iterated Local Search.

The public API is a single function: :func:`specialize`.
"""

from ._ramparils import specialize as _specialize


def specialize(
    strategy: dict[str, str],
    scenario: dict,
) -> dict[str, str]:
    """Specialize a strategy on a set of benchmark instances using ILS.

    Runs Iterated Local Search starting from *strategy*, evaluating
    ``(configuration, instance)`` pairs in parallel, and returns the best
    configuration found within the time budget.

    Results may be cached in SQLite when ``scenario["cache_db"]`` names a file.
    Cache entries are keyed only by the active configuration and instance
    path, so use a separate cache when solver semantics change.

    Args:
        strategy: Initial parameter configuration as ``{name: value}`` strings.
            Must contain every parameter defined in ``scenario["paramfile"]``.
            Values are strings even for numeric parameters (e.g. ``"1.189"``).
        scenario: Tuning scenario as a dict.

            **Required keys:**

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

            **Optional keys** (all have defaults matching the CLI):

            - **run_obj** (*str*, default ``"runtime"``) — ``"runtime"`` or ``"quality"``.
            - **overall_obj** (*str*, default ``"mean"``) — ``"mean"`` or ``"median"``.
            - **test_instance_file** (*str*) — reserved for future use.
            - **approach** (*str*, default ``"focused"``) — ``"focused"``, ``"basic"``, or
              ``"random"``; ``"random"`` is ParamILS's ``pert_rand`` random-restart baseline.
            - **perturbation_strength** (*int*, default ``4``) — neighbourhood steps per perturbation.
            - **restart_probability** (*float*, default ``0.0``) — ParamILS ``p_restart``.
            - **restart_failures** (*int*, default ``0``) — restart the home base after this
              many consecutive rejected local optima.
            - **restart_target** (*str*, default ``"incumbent"``) — ``"incumbent"`` or ``"random"``.
            - **restart_strength** (*int*, default ``0``) — steps applied to the incumbent by a
              restart; ``0`` means ``2 * perturbation_strength``.
            - **acceptance_tolerance** (*float*, default ``0.0``) — accept a worse local optimum
              within this relative margin of the incumbent.
            - **random_probes** (*int*, default ``0``) — ParamILS ``R``: random configurations
              probed before the first descent. ``0`` starts from the supplied strategy only.
            - **initial_fidelity** (*int*, default ``1``) — initial instances per configuration in FocusedILS.
            - **fidelity_step** (*int*, default ``1``) — instances added at each FocusedILS fidelity increase.
            - **bound_multiplier** (*float*, default ``10.0``) — adaptive capping multiplier.
            - **pruning** (*bool*, default ``True``) — enable adaptive capping.
            - **iterative_deepening** (*bool*, default ``False``) — enable iterative deepening.
            - **lambda_n** (*float*, default ``0.5``) — iterative deepening instance-count factor.
            - **lambda_c** (*float*, default ``0.5``) — iterative deepening cutoff-time factor.
            - **lambda_t** (*float*, default ``0.5``) — iterative deepening timeout factor.
            - **cores** (*int*, default ``0``) — parallel workers; ``0`` uses all available cores.
            - **num_run** (*int*, default ``0``) — run index / random seed (reserved).
            - **cache_db** (*str*, default ``":memory:"``) — path to the SQLite cache.
              The default is an in-process cache that is not persisted.
            - **debug** (*bool*, default ``False``) — print new incumbents and scores to stderr.
            - **debug_wrapper** (*bool*, default ``False``) — print every solver invocation.
            - **debug_solver** (*bool*, default ``False``) — print every solver result.
            - **debug_log** (*str*, default ``None``) — write debug output to this file.
            - **error_log** (*str*, default ``None``) — write failed solver runs to this file.

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
        ...         "cache_db":      "/tmp/ramparils_cache.db",
        ...         "cores":         8,
        ...     },
        ... )
        >>> print(result)
        {'alpha': '1.256', 'rho': '0.5'}
    """
    return _specialize(strategy=strategy, scenario=scenario)


__all__ = ["specialize"]
