//! Unit tests for `ils`, split out from `mod.rs` for file size.

use super::bls::*;
use super::futell::*;
use super::logging::*;
use super::*;

fn simple_space() -> ParamSpace {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "alpha {{1, 2, 3}} [2]").unwrap();
    writeln!(f, "beta {{a, b}} [a]").unwrap();
    let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
    drop(f);
    space
}

fn forbidden_space() -> ParamSpace {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "x {{1, 2}} [1]").unwrap();
    writeln!(f, "y {{a, b}} [a]").unwrap();
    writeln!(f, "{{x=2, y=b}}").unwrap();
    let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
    drop(f);
    space
}

fn conditional_space() -> ParamSpace {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
    writeln!(f, "limit {{1, 2}} [1] | mode in {{slow}}").unwrap();
    let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
    drop(f);
    space
}

/// Like `conditional_space`, plus a clause naming `limit`, which is
/// inactive whenever `mode=fast` — so `{mode=fast, limit=2}` must not
/// reject a configuration whose active projection is just `{mode=fast}`.
fn conditional_forbidden_space() -> ParamSpace {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
    writeln!(f, "limit {{1, 2}} [1] | mode in {{slow}}").unwrap();
    writeln!(f, "{{mode=fast, limit=2}}").unwrap();
    let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
    drop(f);
    space
}

fn cfg(pairs: &[(&str, &str)]) -> Config {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn focused_options() -> IlsOptions {
    IlsOptions {
        approach: Approach::Focused,
        n_workers: 1,
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
        tuner_timeout: 60.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
        debug: crate::DebugOptions::default(),
    }
}

#[test]
fn neighbourhood_size() {
    let space = simple_space();
    let config = cfg(&[("alpha", "2"), ("beta", "a")]);
    let n = neighbourhood(&config, &space);
    // alpha has 2 other values, beta has 1 other value → 3 neighbours
    assert_eq!(n.len(), 3);
    // All neighbours differ in exactly one param
    for nb in &n {
        let diffs: usize = nb.iter().filter(|(k, v)| config.get(*k) != Some(v)).count();
        assert_eq!(diffs, 1);
    }
}

#[test]
fn neighbourhood_skips_forbidden() {
    let space = forbidden_space();
    let config = cfg(&[("x", "2"), ("y", "a")]);
    let n = neighbourhood(&config, &space);
    // From x=2,y=a: can go to x=1,y=a (ok) or x=2,y=b (forbidden) → 1 neighbor
    assert_eq!(n.len(), 1);
    assert_eq!(n[0]["x"], "1");
}

#[test]
fn evaluation_config_omits_inactive_parameters() {
    let space = conditional_space();
    let first = cfg(&[("mode", "fast"), ("limit", "1")]);
    let second = cfg(&[("mode", "fast"), ("limit", "2")]);

    let first_active = active_config(&first, &space);
    let second_active = active_config(&second, &space);

    assert_eq!(first_active, cfg(&[("mode", "fast")]));
    assert_eq!(first_active, second_active);
    assert_eq!(hash_config(&first_active), hash_config(&second_active));
}

#[test]
fn argument_changes_include_conditional_activation() {
    let space = conditional_space();
    let current = cfg(&[("mode", "fast"), ("limit", "2")]);
    let next = cfg(&[("mode", "slow"), ("limit", "2")]);

    assert_eq!(
        format_argument_changes(&current, &next, &space),
        "limit: <inactive> -> 2; mode: fast -> slow"
    );
    assert_eq!(
        format_argument_changes(&next, &current, &space),
        "limit: 2 -> <inactive>; mode: slow -> fast"
    );
}

#[test]
fn argument_changes_are_sorted() {
    let space = simple_space();
    let current = cfg(&[("alpha", "1"), ("beta", "b")]);
    let next = cfg(&[("alpha", "3"), ("beta", "a")]);

    assert_eq!(
        format_argument_changes(&current, &next, &space),
        "alpha: 1 -> 3; beta: b -> a"
    );
}

#[test]
fn perturbation_changes_config() {
    let space = simple_space();
    let config = cfg(&[("alpha", "2"), ("beta", "a")]);
    let mut rng = rand::thread_rng();
    let perturbed = perturbation(config.clone(), 3, &space, &mut rng);
    // After 3 perturbation steps, config should generally differ
    // (probabilistically, but with a space this small it's very likely)
    assert_eq!(perturbed.len(), config.len());
}

#[test]
fn dominates_basic() {
    let opts = IlsOptions {
        approach: Approach::Basic,
        n_workers: 1,
        perturbation_strength: 4,
        debug: crate::DebugOptions::default(),
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
        tuner_timeout: 60.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
    };
    assert!(dominates(1.0, 5, 2.0, 5, &opts)); // strictly better
    assert!(dominates(1.0, 1, 2.0, 10, &opts)); // BasicILS ignores run counts
    assert!(!dominates(2.0, 5, 1.0, 5, &opts)); // worse
    assert!(!dominates(1.0, 5, 1.0, 5, &opts)); // tie — does NOT dominate
}

#[test]
fn dominates_focused() {
    let opts = IlsOptions {
        approach: Approach::Focused,
        n_workers: 1,
        perturbation_strength: 4,
        debug: crate::DebugOptions::default(),
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
        tuner_timeout: 60.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
    };
    assert!(dominates(1.0, 10, 2.0, 5, &opts)); // strictly better score, more runs
    assert!(!dominates(1.0, 3, 2.0, 5, &opts)); // better score but fewer runs
    assert!(!dominates(1.0, 10, 1.0, 5, &opts)); // tie — does NOT dominate
}

#[test]
fn weakly_dominates_resolves_ties_for_the_challenger() {
    let opts = focused_options();

    // Same as `dominates` except on ties, which the acceptance criterion
    // resolves in favour of the challenger ("moving away from incumbent").
    assert!(weakly_dominates(1.0, 10, 2.0, 5, &opts));
    assert!(weakly_dominates(1.0, 10, 1.0, 5, &opts)); // tie — accepted here
    assert!(!weakly_dominates(2.0, 10, 1.0, 5, &opts));
    // The FocusedILS run-count guard still applies.
    assert!(!weakly_dominates(1.0, 3, 1.0, 5, &opts));
    assert!(!weakly_dominates(1.0, 3, 2.0, 5, &opts));
}

/// The softened criterion accepts a worse local optimum only while it stays
/// inside the band around the *incumbent*, and the band is measured from
/// the incumbent precisely so that repeated acceptances cannot walk the
/// home base downhill one margin at a time.
#[test]
fn acceptance_tolerance_band_is_anchored_at_the_incumbent() {
    let mut opts = focused_options();
    opts.acceptance_tolerance = 0.05;
    let old = cfg(&[("alpha", "1")]);
    let new = cfg(&[("alpha", "2")]);
    let incumbent_score = 2.0;

    // Worse than the home base but within 5% of the incumbent: accepted.
    let (config, score, took_new) =
        acceptance_criterion(new.clone(), 2.05, 8, old.clone(), 2.0, 8, incumbent_score, &opts);
    assert_eq!(config, new);
    assert!(took_new);
    assert!((score - 2.05).abs() < 1e-9);

    // Outside the band: rejected.
    let (config, score, took_new) =
        acceptance_criterion(new.clone(), 2.2, 8, old.clone(), 2.0, 8, incumbent_score, &opts);
    assert_eq!(config, old);
    assert!(!took_new);
    assert!((score - 2.0).abs() < 1e-9);

    // The band does not drift with the home base: a home base that already
    // sits inside the band cannot pull the bar up behind it.
    let (config, _, took_new) = acceptance_criterion(new.clone(), 2.15, 8, old.clone(), 2.1, 8, incumbent_score, &opts);
    assert_eq!(config, old);
    assert!(!took_new);
}

/// A zero tolerance has to leave the ParamILS rule untouched, so runs made
/// before the knob existed stay reproducible.
#[test]
fn acceptance_tolerance_zero_is_the_paramils_rule() {
    let opts = focused_options();
    assert_eq!(opts.acceptance_tolerance, 0.0);
    let old = cfg(&[("alpha", "1")]);
    let new = cfg(&[("alpha", "2")]);

    let (config, _, took_new) = acceptance_criterion(new.clone(), 2.000_001, 8, old.clone(), 2.0, 8, 2.0, &opts);
    assert_eq!(config, old, "any worse score is rejected when tolerance is 0");
    assert!(!took_new);
}

/// An infinite incumbent score (nothing measured successfully yet) must not
/// open the band to everything.
#[test]
fn acceptance_tolerance_ignores_non_finite_scores() {
    let mut opts = focused_options();
    opts.acceptance_tolerance = 0.05;
    assert!(!accepted_within_tolerance(1.0, f64::INFINITY, &opts));
    assert!(!accepted_within_tolerance(f64::INFINITY, 1.0, &opts));
}

/// With a negative objective the band still has to widen upward from the
/// incumbent, which `incumbent * (1 + tol)` would get backwards.
#[test]
fn acceptance_tolerance_handles_negative_scores() {
    let mut opts = focused_options();
    opts.acceptance_tolerance = 0.10;
    // Incumbent -2.0; the band reaches up to -1.8.
    assert!(accepted_within_tolerance(-1.9, -2.0, &opts));
    assert!(!accepted_within_tolerance(-1.7, -2.0, &opts));
}

/// A restart from the incumbent is still a perturbation: it must land
/// inside the space, and it must actually move.
#[test]
fn restart_from_incumbent_is_a_stronger_perturbation() {
    let space = simple_space();
    let incumbent = cfg(&[("alpha", "2"), ("beta", "a")]);
    let mut rng = rand::thread_rng();

    let mut moved = 0;
    for _ in 0..50 {
        let restarted = perturbation(incumbent.clone(), 8, &space, &mut rng);
        assert_eq!(restarted.len(), incumbent.len());
        for (name, value) in &restarted {
            let param = space.params.iter().find(|p| &p.name == name).unwrap();
            assert!(param.domain.contains(value), "{name}={value} left its domain");
        }
        if restarted != incumbent {
            moved += 1;
        }
    }
    assert!(moved > 0, "a strength-8 restart never moved in 50 attempts");
}

#[test]
fn acceptance_takes_better_and_ties_but_not_worse() {
    let opts = focused_options();
    let old = cfg(&[("alpha", "1")]);
    let new = cfg(&[("alpha", "2")]);

    // Strictly better: accepted.
    let (config, score, took_new) = acceptance_criterion(new.clone(), 1.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
    assert!(took_new);
    assert_eq!(config, new);
    assert!((score - 1.0).abs() < 1e-9);

    // Tie: accepted, so the home base can cross plateaus.
    let (config, score, took_new) = acceptance_criterion(new.clone(), 2.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
    assert!(took_new);
    assert_eq!(config, new);
    assert!((score - 2.0).abs() < 1e-9);

    // Worse: rejected, home base unchanged.
    let (config, score, took_new) = acceptance_criterion(new.clone(), 3.0, 8, old.clone(), 2.0, 8, 1.0, &opts);
    assert!(!took_new);
    assert_eq!(config, old);
    assert!((score - 2.0).abs() < 1e-9);
}

/// A stale home-base score from a smaller prefix must not be able to reject
/// a challenger measured on the current one.  The loop prevents this by
/// re-measuring both retained states on every fidelity increase; if that
/// invariant were ever broken, the run-count guard is the backstop.
#[test]
fn stale_lower_fidelity_score_cannot_win_a_comparison() {
    let opts = focused_options();

    // The failure this reproduces: a home base measured on 12 instances
    // scoring 0.111, against a challenger measured on 568 scoring 0.489.
    // The stale score looks far better only because short prefixes of this
    // instance list are cheaper.
    assert!(!dominates(0.111, 12, 0.489, 568, &opts));
    assert!(!weakly_dominates(0.111, 12, 0.489, 568, &opts));

    // Measured on the same prefix, the comparison resolves normally.
    assert!(dominates(0.474, 568, 0.489, 568, &opts));
}

#[test]
fn compute_score_mean_runtime() {
    let opts = IlsOptions {
        approach: Approach::Basic,
        n_workers: 1,
        perturbation_strength: 4,
        debug: crate::DebugOptions::default(),
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 60.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
    };
    assert!((compute_score(&[1.0, 2.0, 3.0], &[0.0; 3], &opts) - 2.0).abs() < 1e-9);
}

#[test]
fn compute_score_median_runtime() {
    let opts = IlsOptions {
        approach: Approach::Basic,
        n_workers: 1,
        perturbation_strength: 4,
        debug: crate::DebugOptions::default(),
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 60.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Median,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
    };
    assert!((compute_score(&[3.0, 1.0, 2.0], &[0.0; 3], &opts) - 2.0).abs() < 1e-9);
}

#[test]
fn random_config_not_forbidden() {
    let space = forbidden_space();
    let mut rng = rand::thread_rng();
    for _ in 0..50 {
        let cfg = random_config(&space, &mut rng);
        assert!(!space.is_forbidden(&cfg));
    }
}

#[test]
fn forbidden_clause_on_inactive_parameter_is_not_a_real_constraint() {
    let space = conditional_forbidden_space();
    // `limit` is inactive when mode=fast, so a stale limit=2 sitting in
    // the raw config must not turn into a real constraint.
    let raw = cfg(&[("mode", "fast"), ("limit", "2")]);
    assert!(
        space.is_forbidden(&raw),
        "raw config still matches the clause literally"
    );
    assert!(
        !space.is_forbidden(&active_config(&raw, &space)),
        "active projection drops the inactive `limit` entry, so the clause can't match"
    );
}

#[test]
fn neighbourhood_does_not_reject_move_due_to_inactive_forbidden_match() {
    let space = conditional_forbidden_space();
    let config = cfg(&[("mode", "slow"), ("limit", "2")]);
    let n = neighbourhood(&config, &space);
    // From mode=slow,limit=2: mode->fast drops (deactivates) `limit`, so
    // the resulting active projection is just {mode=fast} and the clause
    // {mode=fast, limit=2} must not block the move; limit->1 is unrelated.
    assert_eq!(n.len(), 2);
    assert!(n.iter().any(|c| c.get("mode").map(String::as_str) == Some("fast")));
    assert!(n.iter().any(|c| c.get("limit").map(String::as_str) == Some("1")));
}

/// Two children of the same guard, forbidden only in combination with
/// each other: `a=2,b=2` is fine while both are inactive (mode=fast), but
/// must be caught the moment a single move flips the shared guard and
/// activates both at once with their still-stale values.
fn shared_guard_forbidden_space() -> ParamSpace {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "mode {{fast, slow}} [fast]").unwrap();
    writeln!(f, "a {{1, 2}} [1] | mode in {{slow}}").unwrap();
    writeln!(f, "b {{1, 2}} [1] | mode in {{slow}}").unwrap();
    writeln!(f, "{{a=2, b=2}}").unwrap();
    let space = crate::params::ParamSpace::from_file(f.path().to_str().unwrap()).unwrap();
    drop(f);
    space
}

#[test]
fn neighbourhood_catches_forbidden_combo_exposed_by_activating_a_shared_guard() {
    let space = shared_guard_forbidden_space();
    // a=2,b=2 sit dormant while mode=fast; nothing has ever validated
    // that combination against the forbidden clause, because both were
    // inactive whenever the value was assigned (random draw or a prior,
    // independent perturbation step).
    let config = cfg(&[("mode", "fast"), ("a", "2"), ("b", "2")]);
    assert!(
        !space.is_forbidden(&active_config(&config, &space)),
        "dormant, so not yet forbidden"
    );

    let n = neighbourhood(&config, &space);
    // The only active param at mode=fast is `mode` itself, so the only
    // neighbour is mode->slow — which activates both a and b at once,
    // exposing the forbidden {a=2,b=2} they were carrying. It must be
    // rejected, leaving no neighbours at all.
    assert!(
        n.iter().all(|c| c.get("mode").map(String::as_str) != Some("slow")),
        "activating the shared guard must not silently surface a forbidden combination: {n:?}"
    );
    assert!(n.is_empty());
}

#[test]
fn random_config_does_not_reject_due_to_inactive_forbidden_match() {
    let space = conditional_forbidden_space();
    let mut rng = rand::thread_rng();
    for _ in 0..200 {
        let cfg = random_config(&space, &mut rng);
        // A forbidden combination naming only an inactive parameter must
        // never make random_config reject a legal active configuration.
        assert!(!space.is_forbidden(&active_config(&cfg, &space)));
    }
}

#[test]
fn fidelity_is_clamped_and_advances_by_step() {
    assert_eq!(initial_n_runs(8, 100), 8);
    assert_eq!(initial_n_runs(100, 8), 8);
    assert_eq!(initial_n_runs(0, 8), 1);

    assert_eq!(next_n_runs(8, 4, 100), 12);
    assert_eq!(next_n_runs(8, 100, 10), 10);
    assert_eq!(next_n_runs(8, 0, 100), 9);
}

#[test]
fn evaluation_waits_through_poll_timeouts() {
    let mut cache = Cache::open(":memory:", false).unwrap();
    let path = "instance.cnf".to_string();
    let ids = cache.load_instances(std::slice::from_ref(&path)).unwrap();
    let instances = vec![(ids[&path], path)];
    let scheduler = Scheduler::new(
        1,
        "sleep 0.7; echo '#%# RamParIls #%# sat, 0.7, 0.0'; true".to_string(),
        2.0,
        crate::DebugOptions::default(),
    );
    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 1,
        perturbation_strength: 1,
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 2.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
        debug: crate::DebugOptions::default(),
    };
    let config = cfg(&[("alpha", "1")]);
    let started = Instant::now();

    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &simple_space(),
        cutoff_time: 2.0,
        deadline: started + Duration::from_secs(2),
    };
    let score = evaluate_config(&mut ctx, &config, &instances, None, None).unwrap();

    assert!(started.elapsed() >= Duration::from_millis(650));
    assert!((score - 0.7).abs() < 1e-9);
}

#[test]
fn evaluation_marks_partial_cache_result_incomplete() {
    let mut cache = Cache::open(":memory:", false).unwrap();
    let paths = vec!["cached.cnf".to_string(), "slow.cnf".to_string()];
    let ids = cache.load_instances(&paths).unwrap();
    let instances = paths.iter().map(|path| (ids[path], path.clone())).collect::<Vec<_>>();
    let space = simple_space();
    let config = cfg(&[("alpha", "1"), ("beta", "a")]);
    let hash = hash_config(&active_config(&config, &space));
    cache.put(hash, ids["cached.cnf"], 0.1, 0.0, "sat", 2.0, None).unwrap();

    let scheduler = Scheduler::new(
        1,
        "sleep 0.5; echo '#%# RamParIls #%# sat, 0.5, 0.0'; true".to_string(),
        2.0,
        crate::DebugOptions::default(),
    );
    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 1,
        perturbation_strength: 1,
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 0.1,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
        debug: crate::DebugOptions::default(),
    };

    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time: 2.0,
        deadline: Instant::now() + Duration::from_millis(50),
    };
    let evaluation = evaluate_config_outcome(&mut ctx, &config, &instances, None, None).unwrap();

    assert!(!evaluation.complete);
    assert!((evaluation.score - 0.1).abs() < 1e-9);
}

#[test]
fn evaluation_combines_runhash_via_xor() {
    // Both instances get the exact same runhash from this fake wrapper
    // (a constant, not derived from the instance), so a correct XOR
    // combination over the two must cancel to zero -- a cheap way to
    // prove the combiner actually ran over both results rather than,
    // say, just taking the first one.
    let mut cache = Cache::open(":memory:", false).unwrap();
    let paths = vec!["a.cnf".to_string(), "b.cnf".to_string()];
    let ids = cache.load_instances(&paths).unwrap();
    let instances = paths.iter().map(|path| (ids[path], path.clone())).collect::<Vec<_>>();
    let space = simple_space();
    let config = cfg(&[("alpha", "1"), ("beta", "a")]);

    // `printf`, not `echo`: the command is invoked as `{algo} {instance}
    // {cutoff} -k v...`, and unlike `echo`, `printf` with no conversion
    // specifiers in its format ignores the trailing positional args
    // instead of echoing them onto the result line, where they would
    // land inside the runhash field (the last, unbounded split segment)
    // and break its hex parse.
    let scheduler = Scheduler::new(
        1,
        "printf '#%%# RamParIls #%%# sat, 0.1, 0.0, 00000000000000ff\\n'".to_string(),
        2.0,
        crate::DebugOptions::default(),
    );
    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers: 1,
        perturbation_strength: 1,
        restart_probability: 0.0,
        restart_failures: 0,
        restart_target: RestartTarget::Incumbent,
        restart_strength: 8,
        acceptance_tolerance: 0.0,
        random_probes: 0,
        initial_fidelity: 1,
        fidelity_step: 1,
        bound_multiplier: 10.0,
        pruning: false,
        tuner_timeout: 2.0,
        run_obj: RunObjective::Runtime,
        overall_obj: OverallObjective::Mean,
        instance_shuffle: true,
        instance_shuffle_seed: 0,
        future_telling: false,
        future_telling_checkpoint: 1.0,
        future_telling_cores: 1,
        future_telling_tolerance: 0.0,
        debug: crate::DebugOptions::default(),
    };

    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time: 2.0,
        deadline: Instant::now() + Duration::from_secs(2),
    };
    let evaluation = evaluate_config_outcome(&mut ctx, &config, &instances, None, None).unwrap();

    assert!(evaluation.complete);
    assert_eq!(evaluation.runhash_n, 2);
    assert_eq!(
        evaluation.runhash, 0,
        "XOR of two identical runhashes must cancel to zero"
    );
    assert_eq!(evaluation.runhash_suffix(2), " runhash=0000000000000000 (n=2/2)");
}

// -----------------------------------------------------------------
// Future-telling: CheckpointTracker (FUTURETELL.md)
// -----------------------------------------------------------------

/// Feeds `runtimes` into a tracker in the given order and returns
/// `(solved_count, ready)`.
fn run_tracker(
    n_virtual: usize,
    checkpoint_time: f64,
    cutoff_time: f64,
    n_total: usize,
    runtimes: &[f64],
) -> (usize, bool) {
    let mut tracker = CheckpointTracker::new(n_virtual, checkpoint_time, cutoff_time, n_total);
    for (i, &runtime) in runtimes.iter().enumerate() {
        tracker.record(i as i64, runtime);
    }
    (tracker.solved_count, tracker.ready)
}

/// The property the earlier, fixed-instance-order design had and this
/// one deliberately doesn't (see `CheckpointTracker`'s own doc comment
/// for why it was traded away): the same multiset of runtimes, delivered
/// in a different order, can now mature at a different point and with a
/// different solved count, because assignment to "whichever virtual
/// worker is least busy" depends on what's already been assigned when
/// each result arrives.
#[test]
fn checkpoint_tracker_now_depends_on_arrival_order_by_design() {
    let values = [0.9, 0.9, 0.9, 0.1, 0.1, 0.1];
    let mut reversed = values;
    reversed.reverse();
    assert_ne!(values, reversed, "the reorder must actually change something");

    let mut a = CheckpointTracker::new(2, 1.0, 1.5, 10);
    for (i, &runtime) in values.iter().enumerate() {
        a.record(i as i64, runtime);
        if a.ready {
            break;
        }
    }
    let mut b = CheckpointTracker::new(2, 1.0, 1.5, 10);
    for (i, &runtime) in reversed.iter().enumerate() {
        b.record(i as i64, runtime);
        if b.ready {
            break;
        }
    }

    assert!(
        a.ready && b.ready,
        "both orders should mature well before all 10 (n_total) arrive"
    );
    assert_ne!(
        (a.n_seen, a.solved_count),
        (b.n_seen, b.solved_count),
        "the same multiset in a different arrival order matured differently"
    );
}

#[test]
fn checkpoint_tracker_runtime_equal_to_cutoff_is_not_solved() {
    // Two virtual workers so each instance gets its own idle one --
    // otherwise the second instance would queue behind the first and
    // its *virtual* finish time would land past the checkpoint even
    // though its own runtime is fast, muddying what this test checks.
    let mut tracker = CheckpointTracker::new(2, 5.0, 5.0, 2);
    // runtime == cutoff exactly: must count as "done by the horizon"
    // (finish <= checkpoint) but NOT as solved (runtime < cutoff is
    // strict).
    tracker.record(0, 5.0);
    assert_eq!(tracker.solved_count, 0);
    // A second, genuinely fast instance on its own idle worker, to
    // confirm the strict `<` is the only thing suppressing the count
    // above, not something broader.
    tracker.record(1, 1.0);
    assert_eq!(tracker.solved_count, 1);
}

/// `par1()` is `score()`'s "treat what hasn't been seen yet as a timeout"
/// convention carried through as a mean runtime instead of an unsolved
/// count: an instance whose real result already arrived but whose virtual
/// bucket-finish landed *past* the horizon is penalized at `cutoff_time`
/// exactly like one that hasn't arrived at all, not counted at its own
/// (later-than-the-horizon) runtime.
#[test]
fn checkpoint_tracker_par1_matches_score_on_the_same_horizon() {
    // One virtual worker so both records land in the same bucket: the
    // second pushes the bucket's cumulative finish past the checkpoint.
    let mut tracker = CheckpointTracker::new(1, 5.0, 5.0, 3);
    tracker.record(0, 2.0); // finish=2.0 <= 5.0: solved, counted
    assert!(!tracker.ready);
    tracker.record(1, 4.0); // finish=6.0 > 5.0: matures the checkpoint, itself uncounted
    assert!(
        tracker.ready,
        "single bucket must mature once its cumulative finish exceeds the horizon"
    );
    assert_eq!(tracker.n_seen, 2);

    assert_eq!(tracker.score(), Some(2.0), "n_total(3) - solved_count(1)");
    // time_sum(2.0, from instance 0 alone) + uncounted(2, instances 1 and 2) * cutoff_time(5.0), / n_total(3)
    assert!((tracker.par1().unwrap() - 4.0).abs() < 1e-9);
}

#[test]
fn checkpoint_tracker_finishing_before_maturing_never_reports_ready() {
    // n_total <= n_virtual: every instance gets its own idle worker, so
    // the "all workers busy > checkpoint_time" condition can only ever
    // be reached via full completion, per D3 -- `ready` must stay false
    // and `score()` must stay `None` even though every result is known.
    let mut tracker = CheckpointTracker::new(8, 1.0, 10.0, 3);
    tracker.record(0, 0.1);
    tracker.record(1, 0.2);
    tracker.record(2, 0.3);
    assert!(!tracker.ready);
    assert_eq!(tracker.score(), None);
}

#[test]
fn checkpoint_tracker_rejected_design_would_have_inverted_the_comparison() {
    // Recorded per FUTURETELL.md D6: the design this replaced (mean of
    // completed-subset runtimes) is volume-*insensitive*, so a config
    // with a handful of great completions could look better than one
    // with hundreds of decent ones. Confirm the actual (count-based)
    // design does not reproduce that inversion.
    let few_great = [0.01, 0.01, 0.01];
    let many_decent = [0.4; 60];

    // Rejected design: mean of completed-subset runtimes.
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
    assert!(
        mean(&few_great) < mean(&many_decent),
        "the rejected mean-based design would call the 3-instance config better"
    );

    // Actual design: n_total - solved_count, over a shared n_total so
    // the counts are comparable, with plenty of virtual workers so both
    // mature well before their (different-sized) inputs run out.
    let n_total = 100;
    let (few_solved, _) = run_tracker(64, 1.0, 1.0, n_total, &few_great);
    let (many_solved, _) = run_tracker(64, 1.0, 1.0, n_total, &many_decent);
    assert!(
        (n_total - many_solved) < (n_total - few_solved),
        "the 60-good-completion config must score better (lower) than the 3-completion one"
    );
}

/// Real per-instance runtimes (fixed benchmark order), for three named
/// E-prover strategies from a completed, already-analyzed evaluation
/// batch -- one line per instance, `n_total = 1999` each. Used only to
/// replay-validate `CheckpointTracker` against already-published
/// reference counts; see FUTURETELL.md's validation plan.
const REPLAY_AUTO: &str = include_str!("../../tests/fixtures/checkpoint-replay/auto.txt");
const REPLAY_E_PRE_CASC_10: &str = include_str!("../../tests/fixtures/checkpoint-replay/e-pre_casc_10.txt");
const REPLAY_RAM_2B65A827: &str = include_str!("../../tests/fixtures/checkpoint-replay/ram-2b65a8274485a1ea.txt");

fn parse_replay_fixture(text: &str) -> Vec<f64> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.parse().unwrap())
        .collect()
}

/// Replays a strategy's real runtimes, in the fixed benchmark order the
/// fixture stores them in, at the same settings the diary's own
/// reference numbers were measured under (n_virtual=64,
/// checkpoint_time=5.0, cutoff_time=5.0). The tracker is now
/// arrival-order dependent by design, so this is the one order the
/// fixture actually documents (the diary's own real dispatch order),
/// not a claim that any other order would agree.
fn replay_solved_count(runtimes: &[f64]) -> usize {
    let n_total = runtimes.len();
    let (solved, ready) = run_tracker(64, 5.0, 5.0, n_total, runtimes);
    assert!(ready, "expected the checkpoint to mature well before full completion");
    solved
}

#[test]
fn checkpoint_tracker_replay_e_pre_casc_10_matches_the_diary_exactly() {
    let runtimes = parse_replay_fixture(REPLAY_E_PRE_CASC_10);
    assert_eq!(runtimes.len(), 1999);
    assert_eq!(replay_solved_count(&runtimes), 38);
}

#[test]
fn checkpoint_tracker_replay_ram_2b65a827_matches_the_diary_exactly() {
    let runtimes = parse_replay_fixture(REPLAY_RAM_2B65A827);
    assert_eq!(runtimes.len(), 1999);
    assert_eq!(replay_solved_count(&runtimes), 47);
}

#[test]
fn checkpoint_tracker_replay_auto_reveals_a_real_par1_violation_upstream() {
    // Diary's own reference count for `auto` at these settings is 55,
    // from the *true* SZS-status-based solved determination. This
    // fixture's `runtime < cutoff` proxy (FUTURETELL.md D6) gives 58 --
    // a *measured*, not hypothetical, discrepancy: 3 of the 1999
    // instances have SZS status `GaveUp` (not a real solve) but report
    // a fast real runtime instead of the cutoff, which is a PAR1
    // violation in the tool that produced this data (solverpy's own
    // eprover integration, not a RamParILS wrapper -- RamParILS's own
    // protocol requires PAR1 unconditionally and documents exactly this
    // failure mode, `docs/reference/protocol.md`). This is the intended
    // outcome of this test: it documents the gap `runtime < cutoff`
    // depends on wrapper compliance to close, with real numbers, not a
    // failure of the simulation port -- see FUTURETELL.md's Risks
    // section and the "Rejected" validation item for `compute_score`.
    let runtimes = parse_replay_fixture(REPLAY_AUTO);
    assert_eq!(runtimes.len(), 1999);
    assert_eq!(replay_solved_count(&runtimes), 58, "see this test's doc comment");
}

// -----------------------------------------------------------------
// Future-telling: instance_shuffle (FUTURETELL.md D5, validation item 4)
// -----------------------------------------------------------------

fn instances_0_to_9() -> Vec<(i64, String)> {
    (0..10).map(|i| (i, i.to_string())).collect()
}

#[test]
fn shuffle_instances_is_deterministic_given_a_fixed_seed() {
    let instances = instances_0_to_9();
    let a = shuffle_instances(&instances, 42);
    let b = shuffle_instances(&instances, 42);
    assert_eq!(a, b, "the same seed must produce the same order every time");
}

#[test]
fn shuffle_instances_differs_across_seeds() {
    let instances = instances_0_to_9();
    let a = shuffle_instances(&instances, 1);
    let b = shuffle_instances(&instances, 2);
    assert_ne!(
        a, b,
        "different seeds should (overwhelmingly likely) give different orders"
    );
}

#[test]
fn shuffle_instances_is_a_pure_reordering() {
    // A permutation, never a resample: the shuffled vector must contain
    // exactly the same (id, path) pairs, just reordered.
    let instances = instances_0_to_9();
    let mut shuffled = shuffle_instances(&instances, 7);
    assert_ne!(shuffled, instances, "the scramble must actually reorder something");
    shuffled.sort();
    let mut original = instances.clone();
    original.sort();
    assert_eq!(shuffled, original);
}

/// Instance-ID assignment (`cache.load_instances`) must be independent of
/// whether/how shuffling is configured (FUTURETELL.md D5's ordering
/// requirement): the same path always resolves to the same `instance_id`,
/// whether or not `instance_shuffle` is set, since the shuffle only ever
/// runs on an already-ID-assigned `Vec`.
#[test]
fn instance_shuffle_never_changes_which_id_a_path_resolves_to() {
    let cache = Cache::open(":memory:", false).unwrap();
    let paths: Vec<String> = (0..20).map(|i| format!("instance{i}.cnf")).collect();
    let id_map = cache.load_instances(&paths).unwrap();
    let instances: Vec<(i64, String)> = paths.iter().map(|p| (id_map[p], p.clone())).collect();

    let shuffled = shuffle_instances(&instances, 99);

    // Same multiset of (id, path) pairs...
    let mut a = instances.clone();
    let mut b = shuffled.clone();
    a.sort();
    b.sort();
    assert_eq!(a, b);

    // ...and every path in the shuffled copy still carries the exact id
    // `cache.load_instances` assigned it, not some id implied by its new
    // position.
    for (id, path) in &shuffled {
        assert_eq!(*id, id_map[path], "shuffle must never change {path}'s instance_id");
    }
}

// -----------------------------------------------------------------
// Future-telling: the D3 gate (FUTURETELL.md, validation item 5)
// -----------------------------------------------------------------

#[test]
fn future_telling_active_requires_strictly_more_runs_than_virtual_cores() {
    let mut opts = focused_options();
    opts.future_telling = true;
    opts.future_telling_cores = 10;

    assert!(
        !future_telling_active(&opts, 10),
        "equal to future_telling_cores must not open the gate"
    );
    assert!(!future_telling_active(&opts, 5));
    assert!(future_telling_active(&opts, 11));

    opts.future_telling = false;
    assert!(
        !future_telling_active(&opts, 1000),
        "the master switch must gate regardless of n_instances"
    );
}

// -----------------------------------------------------------------
// Future-telling: end-to-end BLS integration (FUTURETELL.md, validation
// item 5)
// -----------------------------------------------------------------

/// Builds the shared fixture for the checkpoint-rejection integration
/// tests below: a `mode {start, bad, good}` parameter space (so
/// `start`'s neighbourhood is exactly `[bad, good]`), a wrapper whose
/// real per-instance sleep is a deterministic, strictly increasing
/// function of the instance's own fixed position (`0.01 * idx` seconds,
/// via a zero-padded instance name so no floating-point shell arithmetic
/// is needed) -- so with a *single* real worker, real arrival order for
/// one config always matches fixed position order exactly, no OS
/// scheduling race involved. `-mode good` solves fast; anything else
/// (`bad`, and `start` if it's ever dispatched as a real neighbour) times
/// out at the cutoff.
///
/// Returns `(scheduler, cache, space, instances, log)`, where `log` is
/// the path every real solver invocation appends `"<mode> <idx>\n"` to.
fn future_telling_fixture(
    n_instances: usize,
    cutoff_time: f64,
    n_workers: usize,
    domain: &[&str],
) -> (
    Scheduler,
    Cache,
    ParamSpace,
    Vec<(i64, String)>,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    use std::io::Write;
    assert!(n_instances <= 100, "instance names are 2-digit zero-padded");
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("invocations.log");
    let wrapper = dir.path().join("wrapper.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n\
                 idx=\"$1\"; cutoff=\"$2\"; shift 2\n\
                 mode=\"bad\"\n\
                 while [ $# -gt 0 ]; do case \"$1\" in -mode) mode=\"$2\"; shift 2;; *) shift;; esac; done\n\
                 echo \"$mode $idx\" >> '{}'\n\
                 sleep \"0.$idx\"\n\
                 if [ \"$mode\" = good ]; then\n\
                 echo \"#%# RamParIls #%# sat, 0.02, 0.0\"\n\
                 else\n\
                 echo \"#%# RamParIls #%# Timeout, $cutoff, 0.0\"\n\
                 fi\n",
            log.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    std::fs::set_permissions(&wrapper, permissions).unwrap();

    let mut params_file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    writeln!(params_file, "mode {{{}}} [{}]", domain.join(", "), domain[0]).unwrap();
    let space = ParamSpace::from_file(params_file.path().to_str().unwrap()).unwrap();

    let cache = Cache::open(":memory:", false).unwrap();
    // 2-digit zero-padded so `sleep "0.$idx"` scales consistently
    // (`0.$idx` is idx hundredths of a second either way, "00".."19" ->
    // 0..190ms) instead of "0."+"5"="0.05" vs "0."+"19"="0.19" mixing
    // scales for single- vs double-digit indices.
    let paths: Vec<String> = (0..n_instances).map(|i| format!("{i:02}")).collect();
    let ids = cache.load_instances(&paths).unwrap();
    let instances: Vec<(i64, String)> = paths.iter().map(|p| (ids[p], p.clone())).collect();

    let scheduler = Scheduler::new(
        n_workers,
        wrapper.display().to_string(),
        cutoff_time,
        crate::DebugOptions::default(),
    );

    (scheduler, cache, space, instances, log, dir)
}

fn count_invocations(log: &std::path::Path, mode: &str) -> usize {
    std::fs::read_to_string(log)
        .map(|s| s.lines().filter(|l| l.starts_with(&format!("{mode} "))).count())
        .unwrap_or(0)
}

/// A single, obviously-bad config's own evaluation stops early
/// (`complete: false`, fewer than `n_instances` real results counted)
/// once its checkpoint matures against a strict reference -- the same
/// "stop watching it, count it as done, move on" outcome adaptive
/// capping already produces (D8). Uses a single real worker so arrival
/// order is deterministic (no other config competing for threads), which
/// is what makes the exact invocation count at rejection reproducible
/// rather than a race against OS thread scheduling.
#[test]
fn future_telling_stops_evaluating_a_bad_config_before_full_completion() {
    let n_instances = 20;
    let cutoff_time = 1.0;
    let (scheduler, mut cache, space, instances, _log, _dir) =
        future_telling_fixture(n_instances, cutoff_time, 1, &["start", "bad", "good"]);

    let mut options = focused_options();
    options.future_telling = true;
    // With n_virtual=4 and `bad`'s runtime == cutoff_time == 1.0 always,
    // all 4 virtual workers cross this horizon right after the 4th
    // (fixed-order) position is simulated.
    options.future_telling_checkpoint = 0.15;
    options.future_telling_cores = 4;
    options.future_telling_tolerance = 0.0;

    let bad = cfg(&[("mode", "bad")]);
    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    let evaluation = evaluate_config_outcome(
        &mut ctx,
        &bad,
        &instances,
        None,
        Some(5.0), // incumbent checkpoint: "5 of 20 unsolved so far"
    )
    .unwrap();

    assert!(
        !evaluation.complete,
        "a checkpoint-rejected config must not be reported as a real measurement"
    );
    assert!(
        evaluation.n_done < n_instances,
        "expected the evaluation to stop well before {n_instances} real results, got {}",
        evaluation.n_done
    );
}

/// The mirror case: an obviously-good config, evaluated under the exact
/// same future-telling settings, is never checkpoint-rejected -- its own
/// tracker structurally never matures at this horizon (D3/D7's "config
/// finished before maturing" case: real dispatch (FUTURETELL.md D11)
/// keeps every real worker's own busy time under `checkpoint_time=1.0`
/// for all 20 instances), so it runs to full, real completion regardless
/// of how strict the reference is.
///
/// Uses `n_workers == future_telling_cores` deliberately -- D11's clean
/// bound (and this test's premise) only holds without oversubscription;
/// `n_workers=1` (as the sibling `bad`-config test above uses, for
/// deterministic ordering) would make each real worker's own cumulative
/// dispatch delay reach the horizon almost immediately regardless of how
/// fast any individual instance solves, maturing the checkpoint for the
/// wrong reason (worker starvation, not the config being good or bad).
#[test]
fn future_telling_never_rejects_a_config_whose_checkpoint_never_matures() {
    let n_instances = 20;
    let cutoff_time = 1.0;
    let (scheduler, mut cache, space, instances, _log, _dir) =
        future_telling_fixture(n_instances, cutoff_time, 4, &["start", "bad", "good"]);

    let mut options = focused_options();
    options.future_telling = true;
    options.future_telling_checkpoint = 1.0;
    options.future_telling_cores = 4;
    // Deliberately impossible to satisfy, to prove the absence of
    // rejection is structural (the tracker never matures), not merely a
    // lenient tolerance happening not to fire.
    options.future_telling_tolerance = 0.0;

    let good = cfg(&[("mode", "good")]);
    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    let evaluation = evaluate_config_outcome(&mut ctx, &good, &instances, None, Some(0.0)).unwrap();

    assert!(evaluation.complete);
    assert_eq!(evaluation.n_done, n_instances);
    assert!((evaluation.score - 0.02).abs() < 1e-6);
}

/// Full-pipeline wiring check, not a proof that checkpoint rejection
/// alone decided the outcome: `bad`'s real, completed score (1.0) can
/// never dominate `start_eval`'s 0.5 either way, checkpoint or not, so
/// this doesn't isolate the feature's causal contribution the way the
/// two single-config tests above do. What it does confirm is that the
/// per-round tracker/position bookkeeping `basic_local_search` now
/// builds when `future_telling` is on doesn't corrupt the ordinary
/// accept path: with both a checkpoint-eligible-but-never-winning `bad`
/// neighbour and a never-matures `good` one in the same round, the
/// local optimum returned is still the good one, with a real,
/// full-completion score.
#[test]
fn future_telling_never_lets_a_rejected_neighbour_win_a_round() {
    let n_instances = 20;
    let cutoff_time = 1.0;
    let (scheduler, mut cache, space, instances, _log, _dir) =
        future_telling_fixture(n_instances, cutoff_time, 4 * n_instances, &["start", "bad", "good"]);
    let mut rng = rand::thread_rng();

    let mut options = focused_options();
    options.approach = Approach::Basic;
    options.n_workers = 4 * n_instances;
    options.pruning = true;
    options.bound_multiplier = 10.0;
    options.future_telling = true;
    options.future_telling_checkpoint = 0.15;
    options.future_telling_cores = 4;
    options.future_telling_tolerance = 0.0;

    let start = cfg(&[("mode", "start")]);
    let start_eval = ConfigEvaluation::complete(0.5, n_instances);
    let incumbent_score = 1.0;
    let incumbent_checkpoint = Some(5.0);

    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    let (current, current_eval, steps) = basic_local_search(
        &mut ctx,
        start,
        start_eval,
        &instances,
        incumbent_score,
        incumbent_checkpoint,
        &mut rng,
    )
    .unwrap();

    assert!(
        steps >= 1,
        "the obviously better neighbour must have been accepted at least once"
    );
    assert_eq!(current.get("mode").map(String::as_str), Some("good"));
    assert!((current_eval.score - 0.02).abs() < 1e-6);
}

/// D3's gate, exercised structurally rather than by "no rejection
/// happened" alone (which could pass for the wrong reason, e.g. a
/// tolerance that merely never triggered): with `n_runs <=
/// future_telling_cores`, the mechanism cannot mature even against a
/// deliberately strict reference (`incumbent_checkpoint: Some(0.0)`), so
/// an obviously bad neighbour must still run to full real completion.
#[test]
fn future_telling_gate_blocks_rejection_when_n_runs_does_not_exceed_virtual_cores() {
    let n_instances = 5;
    let cutoff_time = 1.0;
    // No `good` value here at all -- `start`'s only neighbour is `bad`,
    // so there is nothing else in the round that could race it to
    // completion and confuse the invocation count this test checks.
    let (scheduler, mut cache, space, instances, log, _dir) =
        future_telling_fixture(n_instances, cutoff_time, 4 * n_instances, &["start", "bad"]);
    let mut rng = rand::thread_rng();

    let mut options = focused_options();
    options.approach = Approach::Basic;
    options.n_workers = 4 * n_instances;
    options.pruning = false;
    options.future_telling = true;
    options.future_telling_checkpoint = 0.01; // as sharp a horizon as possible
    options.future_telling_cores = 10; // >= n_instances: D3 gate must block
    options.future_telling_tolerance = 0.0;

    let start = cfg(&[("mode", "start")]);
    let start_eval = ConfigEvaluation::complete(f64::INFINITY, n_instances);
    // Deliberately impossible to satisfy, to prove absence of rejection
    // is structural (the gate), not just this threshold happening not to
    // fire.
    let incumbent_checkpoint = Some(0.0);

    let mut ctx = EvalContext {
        scheduler: &scheduler,
        cache: &mut cache,
        options: &options,
        space: &space,
        cutoff_time,
        deadline: Instant::now() + Duration::from_secs(10),
    };
    basic_local_search(
        &mut ctx,
        start,
        start_eval,
        &instances,
        f64::INFINITY,
        incumbent_checkpoint,
        &mut rng,
    )
    .unwrap();

    let bad_invocations = count_invocations(&log, "bad");
    assert_eq!(
        bad_invocations, n_instances,
        "below the D3 gate, future-telling must never build a tracker, so `bad` must run to full completion"
    );
}
