//! Integration tests using the eprover example in examples/eprover/.
//!
//! Requires `grackle-ramparils` and the `solverpy_grackle` Python package to be
//! installed (they are part of the Grackle toolchain).  The test sets
//! `SOLVERPY_BENCHMARKS` to point at the bundled `bushy010/` problems so no
//! external data is needed.

use std::path::PathBuf;

use ramparils::{
    cache::Cache,
    ils::{self, Approach, IlsOptions, RestartTarget},
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
    assert_eq!(space.params.len(), 21, "expected 21 parameters");
}

#[test]
fn parse_eprover_params_conditionals() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();

    // 10 params carry conditions: tord_weight, tord_const (on tord=KBO6), and
    // heur1..4/freq1..4 (on slots ∈ {1,2,3,4}/{2,3,4}/{3,4}/{4} respectively)
    let n_cond = space.params.iter().filter(|p| p.condition.is_some()).count();
    assert_eq!(n_cond, 10);

    // tord_weight depends on tord=KBO6
    let tw = space.params.iter().find(|p| p.name == "tord_weight").unwrap();
    let c = tw.condition.as_ref().unwrap();
    assert_eq!(c.parent, "tord");
    assert_eq!(c.allowed_values, vec!["KBO6"]);

    // heur1/freq1 depend on slots ∈ {1,2,3,4}
    let h1 = space.params.iter().find(|p| p.name == "heur1").unwrap();
    let c1 = h1.condition.as_ref().unwrap();
    assert_eq!(c1.parent, "slots");
    assert_eq!(c1.allowed_values, vec!["1", "2", "3", "4"]);

    // heur4/freq4 depend on slots ∈ {4} only
    let h4 = space.params.iter().find(|p| p.name == "heur4").unwrap();
    let c4 = h4.condition.as_ref().unwrap();
    assert_eq!(c4.parent, "slots");
    assert_eq!(c4.allowed_values, vec!["4"]);
}

#[test]
fn parse_eprover_params_sel_domain() {
    let dir = eprover_dir();
    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();

    let sel = space.params.iter().find(|p| p.name == "sel").unwrap();
    assert_eq!(sel.domain.len(), 8);
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
    assert!(
        !active.contains(&"tord_weight"),
        "tord_weight should be inactive with tord=LPO4"
    );
    assert!(
        !active.contains(&"tord_const"),
        "tord_const should be inactive with tord=LPO4"
    );

    // slots default = 0 → every heurN/freqN slot (all guarded by slots >= 1)
    // is inactive at the default configuration.
    for name in ["heur1", "freq1", "heur2", "freq2", "heur3", "freq3", "heur4", "freq4"] {
        assert!(!active.contains(&name), "{name} should be inactive with slots=0");
    }

    // Raising slots to 4 must activate exactly the slots the file's own
    // conditions promise: heur1/freq1 (slots∈{1,2,3,4}), heur2/freq2
    // (slots∈{2,3,4}), heur3/freq3 (slots∈{3,4}), heur4/freq4 (slots∈{4}).
    let mut four_slots = default.clone();
    four_slots.insert("slots".to_string(), "4".to_string());
    assert!(!space.is_forbidden(&four_slots));
    let active_at_4: Vec<&str> = space.active_params(&four_slots).iter().map(|p| p.name.as_str()).collect();
    for name in ["heur1", "freq1", "heur2", "freq2", "heur3", "freq3", "heur4", "freq4"] {
        assert!(active_at_4.contains(&name), "{name} should be active with slots=4");
    }
}

// ── ILS integration test ─────────────────────────────────────────────────────

#[test]
fn run_ils_eprover() {
    let dir = eprover_dir();
    let benchmarks_dir = dir.join("bushy010");
    let wrapper = dir.join("grackle-eprover.sh");

    // Pass SOLVERPY_BENCHMARKS inline so the wrapper can find the .p files.
    let algo = format!("SOLVERPY_BENCHMARKS={} {}", benchmarks_dir.display(), wrapper.display(),);

    let space = ParamSpace::from_file(dir.join("params-eprover.txt").to_str().unwrap()).unwrap();
    let instance_paths = scenario::load_instances(dir.join("instances-bushy010.txt").to_str().unwrap()).unwrap();

    let mut cache = Cache::open(":memory:", false).unwrap();
    let id_map = cache.load_instances(&instance_paths).unwrap();
    let instances: Vec<(i64, String)> = instance_paths.iter().map(|p| (id_map[p], p.clone())).collect();

    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 4,
        perturbation_strength: 4,
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: true,
        tuner_timeout: 15.0,
        run_obj: RunObjective::Quality,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
        debug: ramparils::DebugOptions::default(),
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
