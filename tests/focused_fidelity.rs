//! Regression test for FocusedILS fidelity consistency.
//!
//! FocusedILS grows `n_runs` during a run, and a score is only meaningful
//! relative to the instance prefix it was measured on. Both states the ILS
//! carries across iterations — the incumbent and the ILS home base — must
//! therefore be re-measured whenever the fidelity increases.
//!
//! This lives in its own integration-test binary because it installs a debug
//! log, which is process-global state.

use std::io::Write;

use ramparils::cache::Cache;
use ramparils::ils::{Approach, IlsOptions, RestartTarget};
use ramparils::params::ParamSpace;
use ramparils::scenario::{OverallObjective, RunObjective};

const N_INSTANCES: usize = 20;

/// A deterministic fake solver whose runtime depends only on the instance:
/// `instNN` takes `NN / 100` seconds, regardless of the configuration.
///
/// That makes every configuration score identically at any given fidelity, and
/// makes the score at fidelity `n` exactly `expected_prefix_mean(n)` — which is
/// strictly increasing in `n`. A score carried over from a smaller prefix is
/// therefore always detectably too low.
const FAKE_SOLVER: &str = r##"f() { n=$(echo "$1" | tr -dc '0-9'); awk -v n="$n" 'BEGIN{printf "#%%# RamParIls #%%# sat, %.4f, 0.0\n", n/100}'; }; f"##;

/// Mean runtime over the first `n` instances: (0.01 + 0.02 + … + 0.01n) / n.
fn expected_prefix_mean(n: usize) -> f64 {
    (1..=n).map(|i| i as f64 / 100.0).sum::<f64>() / n as f64
}

fn parameter_space() -> (tempfile::NamedTempFile, ParamSpace) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file, "alpha {{1, 2, 3}} [2]").unwrap();
    writeln!(file, "beta {{a, b}} [a]").unwrap();
    let space = ParamSpace::from_file(file.path().to_str().unwrap()).unwrap();
    (file, space)
}

#[test]
fn fidelity_increases_remeasure_both_retained_states() {
    let log = tempfile::NamedTempFile::new().unwrap();
    ramparils::init_log_file(log.path().to_str().unwrap()).unwrap();

    let (_params_file, space) = parameter_space();
    let mut cache = Cache::open(":memory:", false).unwrap();
    let paths: Vec<String> = (1..=N_INSTANCES).map(|i| format!("inst{i:02}")).collect();
    let ids = cache.load_instances(&paths).unwrap();
    let instances: Vec<(i64, String)> =
        paths.iter().map(|path| (ids[path], path.clone())).collect();

    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 4,
        perturbation_strength: 2,
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 2,
        fidelity_step: 2,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 8.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        debug: ramparils::DebugOptions {
            main: true,
            wrapper: false,
            solver: false,
        },
    };

    ramparils::ils::run(
        Some(space.default_config()),
        &options,
        &space,
        &instances,
        FAKE_SOLVER,
        1.0,
        &mut cache,
    )
    .unwrap();

    ramparils::close_log_file();
    let text = std::fs::read_to_string(log.path()).unwrap();

    let steps: Vec<(usize, f64, f64)> = text
        .lines()
        .filter_map(|line| {
            let rest = line.split("ils: n_runs increased to ").nth(1)?;
            let fidelity: usize = rest.split('/').next()?.parse().ok()?;
            let incumbent: f64 = rest
                .split("incumbent_score=")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            let home_base: f64 = rest
                .split("home_base_score=")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            Some((fidelity, incumbent, home_base))
        })
        .collect();

    assert!(
        steps.len() >= 5,
        "expected several fidelity increases, got {}:\n{text}",
        steps.len()
    );

    for (fidelity, incumbent, home_base) in &steps {
        let expected = expected_prefix_mean(*fidelity);
        assert!(
            (incumbent - expected).abs() < 1e-4,
            "incumbent score {incumbent} at fidelity {fidelity} is not measured \
             on that prefix (expected {expected})",
        );
        // The regression: a home base left at an older, smaller prefix scores
        // strictly lower than one measured here, and would never be displaced.
        assert!(
            (home_base - expected).abs() < 1e-4,
            "home base score {home_base} at fidelity {fidelity} is stale — \
             expected {expected}, the mean over the first {fidelity} instances",
        );
    }

    // The run should have climbed rather than stalling at the initial fidelity.
    let reached = steps.last().unwrap().0;
    assert!(
        reached > options.initial_fidelity,
        "fidelity never advanced past {}",
        options.initial_fidelity
    );

    // Home-base replacements are logged, so the perturbation centre can be
    // followed without reconstructing it from the acceptance rule. Every
    // configuration ties under this fake solver, and the acceptance criterion
    // resolves ties in favour of the challenger, so the home base should move
    // freely here.
    let home_base_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("ils: new home base:"))
        .collect();
    assert!(
        !home_base_lines.is_empty(),
        "no home-base replacement was logged:\n{text}"
    );

    for line in &home_base_lines {
        let changes = line
            .split(" changes: ")
            .nth(1)
            .unwrap_or_else(|| panic!("home-base line carries no parameter diff: {line}"));
        assert!(
            !changes.trim().is_empty(),
            "home-base line logged an empty diff, so nothing actually changed: {line}"
        );
        assert!(
            line.contains("hash=") && line.contains("score=") && line.contains("instances="),
            "home-base line is missing an identifying field: {line}"
        );
    }
}
