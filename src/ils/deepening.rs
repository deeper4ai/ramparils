//! Iterative-deepening wrapper around [`super::run`].

use anyhow::Result;
use std::time::Instant;

use crate::cache::Cache;
use crate::params::{Config, ParamSpace};

use super::IlsOptions;

/// Mirrors `iterative_deepening_ils()` from `param_ils_2_3_run.rb` (lines 809–856,
/// 1795–1837).  Builds an exponential schedule over instance count, cutoff time,
/// and per-phase timeout controlled by λ_n, λ_c, λ_t (defaults: 0.5 each).
/// Each phase seeds the next with its incumbent.
#[allow(clippy::too_many_arguments)]
pub fn iterative_deepening_ils(
    initial: Option<Config>,
    options: &IlsOptions,
    space: &ParamSpace,
    instances: &[(i64, String)],
    algo: &str,
    cutoff_time: f64,
    cache: &mut Cache,
    lambda_n: f64,
    lambda_c: f64,
    lambda_t: f64,
) -> Result<(Config, f64)> {
    let n_total = instances.len();
    let eps = 1e-6;

    // Number of phases: enough that the earliest phase starts near 1 instance / 1s cutoff.
    let num_depths = {
        let raw = if lambda_n < 1.0 - eps {
            ((n_total as f64).ln() / (1.0 / lambda_n).ln()).ceil() as usize + 1
        } else {
            (cutoff_time.ln().max(0.0) / (1.0 / lambda_c).ln()).ceil() as usize + 1
        };
        raw.max(1)
    };

    // Build schedule: (n_instances, phase_cutoff, phase_t_budget_from_start)
    let schedule: Vec<(usize, f64, f64)> = (0..num_depths)
        .map(|i| {
            let exp = (num_depths - 1 - i) as f64;
            let n = ((n_total as f64) * lambda_n.powf(exp)).ceil() as usize;
            let n = n.max(1).min(n_total);
            let c = (cutoff_time * lambda_c.powf(exp)).ceil();
            let t = options.tuner_timeout * lambda_t.powf(exp);
            (n, c, t)
        })
        .collect();

    if options.debug.main {
        crate::debug_line(
            options.debug.main,
            &format!(
                "[{:8.2}s] id: {} phases  λ_n={lambda_n} λ_c={lambda_c} λ_t={lambda_t}",
                crate::t(),
                num_depths
            ),
        );
        for (i, (n, c, t)) in schedule.iter().enumerate() {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id:   phase {} n={n} cutoff={c:.1}s timeout={t:.1}s",
                    crate::t(),
                    i + 1
                ),
            );
        }
    }

    let start = Instant::now();
    let mut current_initial = initial;
    let mut best: Option<(Config, f64)> = None;

    for (depth, (n, c, t)) in schedule.iter().enumerate() {
        if crate::interrupted() {
            break;
        }
        let elapsed = start.elapsed().as_secs_f64();
        let phase_remaining = t - elapsed;
        if phase_remaining <= 0.0 {
            if options.debug.main {
                crate::debug_line(
                    options.debug.main,
                    &format!(
                        "[{:8.2}s] id: phase {}/{} skipped (budget exhausted)",
                        crate::t(),
                        depth + 1,
                        num_depths
                    ),
                );
            }
            break;
        }

        if options.debug.main {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id: starting phase {}/{} n={n} cutoff={c:.1}s remaining={phase_remaining:.1}s",
                    crate::t(),
                    depth + 1,
                    num_depths
                ),
            );
        }

        let mut phase_options = options.clone();
        phase_options.tuner_timeout = phase_remaining;

        let (inc, score) = super::run(
            current_initial.take(),
            &phase_options,
            space,
            &instances[..*n],
            algo,
            *c,
            cache,
        )?;

        if options.debug.main {
            crate::debug_line(
                options.debug.main,
                &format!(
                    "[{:8.2}s] id: phase {}/{} done — score={score:.6}",
                    crate::t(),
                    depth + 1,
                    num_depths
                ),
            );
        }

        current_initial = Some(inc.clone());
        best = Some((inc, score));
    }

    best.ok_or_else(|| anyhow::anyhow!("no phases ran (budget already exhausted)"))
}
