//! Integration tests using the eprover example in examples/eprover/.
//!
//! Requires `grackle-ramparils` and the `solverpy_grackle` Python package to be
//! installed (they are part of the Grackle toolchain).  The test sets
//! `SOLVERPY_BENCHMARKS` to point at the bundled `bushy010/` problems so no
//! external data is needed.

use std::path::PathBuf;

use ramparils::{
    cache::Cache,
    ils::{self, Approach, IlsOptions},
    params::ParamSpace,
    scenario::{self, OverallObjective, RunObjective},
};

fn eprover_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/eprover")
}

// ── params tests ─────────────────────────────────────────────────────────────

#[test]
fn parse_eprover_params_count() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();
    assert_eq!(space.params.len(), 34, "expected 34 parameters");
}

#[test]
fn parse_eprover_params_conditionals() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();

    // 5 params carry conditions (tord_weight, tord_const, heur1, heur2, heur3)
    let n_cond = space.params.iter().filter(|p| p.condition.is_some()).count();
    assert_eq!(n_cond, 5);

    // tord_weight depends on tord=KBO6
    let tw = space.params.iter().find(|p| p.name == "tord_weight").unwrap();
    let c = tw.condition.as_ref().unwrap();
    assert_eq!(c.parent, "tord");
    assert_eq!(c.allowed_values, vec!["KBO6"]);

    // heur1 depends on slots ∈ {2,3,4}
    let h1 = space.params.iter().find(|p| p.name == "heur1").unwrap();
    let c1 = h1.condition.as_ref().unwrap();
    assert_eq!(c1.parent, "slots");
    assert_eq!(c1.allowed_values, vec!["2", "3", "4"]);
}

#[test]
fn parse_eprover_params_sel_domain() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();

    let sel = space.params.iter().find(|p| p.name == "sel").unwrap();
    assert_eq!(sel.domain.len(), 9);
    assert_eq!(sel.default, "SelectMaxLComplexAvoidPosPred");
    assert!(sel.condition.is_none());
}

#[test]
fn parse_eprover_params_no_forbidden() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();
    assert!(space.forbidden.is_empty());
}

#[test]
fn parse_eprover_params_default_active() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();
    let default = space.default_config();
    assert!(!space.is_forbidden(&default));

    let active: Vec<&str> = space.active_params(&default).iter().map(|p| p.name.as_str()).collect();

    // tord default = LPO4 → tord_weight and tord_const are inactive
    assert!(!active.contains(&"tord_weight"), "tord_weight should be inactive with tord=LPO4");
    assert!(!active.contains(&"tord_const"), "tord_const should be inactive with tord=LPO4");

    // slots default = 4 → heur1 (slots∈{2,3,4}), heur2 (slots∈{3,4}), heur3 (slots∈{4}) all active
    assert!(active.contains(&"heur1"), "heur1 should be active with slots=4");
    assert!(active.contains(&"heur2"), "heur2 should be active with slots=4");
    assert!(active.contains(&"heur3"), "heur3 should be active with slots=4");
}

// ── ILS integration test ─────────────────────────────────────────────────────

#[test]
fn run_ils_eprover() {
    let dir = eprover_dir();
    let benchmarks_dir = dir.join("bushy010");
    let wrapper = dir.join("grackle-eprover.sh");

    // Pass SOLVERPY_BENCHMARKS inline so the wrapper can find the .p files.
    let algo = format!(
        "SOLVERPY_BENCHMARKS={} {}",
        benchmarks_dir.display(),
        wrapper.display(),
    );

    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();
    let instance_paths =
        scenario::load_instances(dir.join("instances-bushy010.txt").to_str().unwrap()).unwrap();

    let mut cache = Cache::open(":memory:", false).unwrap();
    let id_map = cache.load_instances(&instance_paths).unwrap();
    let instances: Vec<(i64, String)> = instance_paths
        .iter()
        .map(|p| (id_map[p], p.clone()))
        .collect();

    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 4,
        perturbation_strength: 4,
        bound_multiplier: 10.0,
        pruning: true,
        tuner_timeout: 15.0,
        run_obj: RunObjective::Quality,
        overall_obj: OverallObjective::Mean,
        debug: false,
        debug_wrapper: false,
        debug_solver: false,
    };

    let initial = space.default_config();
    let (result, _score) = ils::run(
        Some(initial),
        &options,
        &space,
        &instances,
        &algo,
        1.0, // cutoff_time
        &mut cache,
    )
    .unwrap();

    // The result must be a valid, non-forbidden configuration with all values in domain.
    assert!(!space.is_forbidden(&result));
    let active = space.active_params(&result);
    assert!(!active.is_empty());
    for param in &active {
        let val = result.get(&param.name).unwrap();
        assert!(
            param.domain.contains(val),
            "param {} has value '{}' outside its domain {:?}",
            param.name,
            val,
            param.domain
        );
    }
}
