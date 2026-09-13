//! Debug-log lines for incumbent/home-base changes, and the parameter-diff
//! formatting behind them.

use anyhow::Result;
use std::collections::BTreeSet;

use crate::cache::hash_config;
use crate::params::{Config, ParamSpace, config_to_yaml};

use super::active_config;
use super::bls::ConfigEvaluation;

pub(super) fn log_incumbent(
    enabled: bool,
    incumbent: &Config,
    eval: &ConfigEvaluation,
    n_runs: usize,
    space: &ParamSpace,
) -> Result<()> {
    if !enabled {
        return Ok(());
    }
    let hash = hash_config(&active_config(incumbent, space));
    let score = eval.score;
    let runhash = eval.runhash_suffix(n_runs);
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new incumbent: hash={hash:016x} score={score:.6} instances={n_runs}{runhash}",
            crate::t()
        ),
    );
    crate::debug_block(true, &config_to_yaml(incumbent)?);
    Ok(())
}

/// Log a replacement of the ILS home base — the configuration the next
/// perturbation starts from (`last_lm`).
///
/// The home base is not the incumbent: the incumbent is the best configuration
/// found, the home base is where the search currently *is*. Only the home base
/// is perturbed, so a home base that stops moving turns the ILS into repeated
/// random restarts from a fixed ball regardless of what the incumbent does —
/// which is exactly what a reader of the log needs to be able to check.
///
/// Unlike a new incumbent this is logged as a single line with the parameter
/// diff against the previous home base, not a full configuration block: the
/// home base can change every round, and the diff is enough to replay the
/// trajectory. Replacements with no *effective* change (a differing value on a
/// parameter whose guard is off) produce an empty diff and are not logged.
pub(super) fn log_home_base(
    enabled: bool,
    previous: &Config,
    home_base: &Config,
    score: &ConfigEvaluation,
    n_runs: usize,
    space: &ParamSpace,
) {
    if !enabled {
        return;
    }
    let changes = format_argument_changes(previous, home_base, space);
    if changes.is_empty() {
        return;
    }
    let hash = hash_config(&active_config(home_base, space));
    let runhash = score.runhash_suffix(n_runs);
    crate::debug_line(
        true,
        &format!(
            "[{:8.2}s] ils: new home base: hash={hash:016x} score={} instances={n_runs}{runhash} changes: {changes}",
            crate::t(),
            score.display(n_runs)
        ),
    );
}

pub(super) fn format_argument_changes(current: &Config, next: &Config, space: &ParamSpace) -> String {
    let current = active_config(current, space);
    let next = active_config(next, space);
    let names: BTreeSet<&str> = current.keys().chain(next.keys()).map(String::as_str).collect();

    names
        .into_iter()
        .filter_map(|name| {
            let before = current.get(name).map(String::as_str);
            let after = next.get(name).map(String::as_str);
            (before != after).then(|| {
                format!(
                    "{name}: {} -> {}",
                    before.unwrap_or("<inactive>"),
                    after.unwrap_or("<inactive>")
                )
            })
        })
        .collect::<Vec<_>>()
        .join("; ")
}
