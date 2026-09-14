# Code Review Findings (2026-09-14)

Findings from `/code-review main branch, all current code in src/`. The
policy-violation finding (internal experiment name in a public repo) and two
`futell`-related findings (Python bindings' legacy-key handling, checkpoint
fallback bucket selection) are fixed and committed; these five are the
correctness bugs still open.

## 1. `src/cache.rs:350` — cache adaptation ignores non-"timeout" failure statuses

Cache write/read cutoff-adaptation only special-cases the literal status
`"timeout"`, not `ResourceOut`/`GaveUp`/other non-success statuses that share
PAR1 semantics (runtime always == cutoff per `docs/reference/protocol.md`).

A config gets `ResourceOut` at cutoff=5s (cached runtime=5.0). A later query
at requested_cutoff=30s: `adapt_cached_result`'s
`eq_ignore_ascii_case("timeout")` check is false for `"ResourceOut"`, and the
`cached.runtime(5.0) > requested_cutoff(30.0)` fallback is also false, so the
stale result is returned unchanged — reporting the config as tested to 30s
when it was only tested to 5s. Symmetrically, `Cache::put`'s `should_write`
only allows overwriting when `existing_status` is `"timeout"`, so a later
genuine success after an earlier `ResourceOut`/`GaveUp` is silently dropped
and the stale failure kept forever.

## 2. `src/params.rs:207` — forbidden-combination check uses raw config, not active projection

`validate_config` calls `self.is_forbidden(config)` on the raw config,
unlike `neighbourhood()` (`src/ils/bls.rs:140`), which correctly checks
`is_forbidden(&active_config(...))`.

Given a conditional parameter that's inactive under the current mode, an
`initial_config` that's legal (the inactive param's value doesn't matter)
can still match a forbidden combo on the raw config and get rejected with
"initial configuration matches a forbidden parameter combination", even
though `random_config()`/`neighbourhood()` would treat it as fine.

## 3. `src/ils/deepening.rs:36` — division by zero when `lambda_c == 1.0`

`num_depths` divides by `(1.0 / lambda_c).ln()` with no validation that
`lambda_c < 1`. `scenario.rs` never validates `lambda_n`/`lambda_c`/
`lambda_t` ranges despite doc comments requiring `0 < λ ≤ 1`.

`iterative_deepening: true` with `lambda_n: 1.0, lambda_c: 1.0` skips the
first branch and computes `cutoff_time.ln() / ln(1.0)` = infinity;
`.ceil() as usize` saturates and `+ 1` overflows — panicking in debug or
wrapping/masking to a degenerate single-phase schedule in release.

## 4. `src/ils/bls.rs:551` (and `:911`) — adaptive-capping budget assumes non-negative score

`bound_multiplier * incumbent_score * n_instances` is only a valid upper
bound when `incumbent_score` is non-negative; for a quality objective where
lower/negative is better, the budget becomes large-negative and pruning
stops firing. Duplicated in `SingleConfigCollector::check_capping` at line
911.

`run_obj: quality` with `incumbent_score = -50` gives
`budget = 10.0 * -50 * n_instances`, a large negative number that a
handful of partial-sum instance qualities almost never fall below, so
`check_capping` never returns true and bad neighbours run to completion
instead of being pruned.

## 5. `src/db.rs:68` — `is_solved()` does exact case-sensitive status match

Contradicts the module's own doc that RamParILS stores wrapper status
verbatim and never interprets it.

A solver wrapper reports a success status in different casing or with
incidental whitespace than the hardcoded 7-entry list; `SOLVED_STATUSES.
contains()` does exact equality, so `ramparils db solved` silently reports
a genuine success as unsolved.
