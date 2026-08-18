//! PyO3 bindings — only compiled with `--features python` (used by maturin).

use std::collections::HashMap;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods};

use crate::cache::Cache;
use crate::ils;
use crate::params::ParamSpace;
use crate::scenario::{OverallObjective, RunObjective, Scenario};

/// Specialize `strategy` on the instance set defined in `scenario`.
///
/// `scenario` is a Python dict — see `ramparils/__init__.py` for the full schema.
///
/// Returns the improved strategy as a `dict[str, str]`.
#[pyfunction]
#[pyo3(signature = (strategy, scenario))]
fn specialize(strategy: HashMap<String, String>, scenario: &Bound<'_, PyDict>) -> PyResult<HashMap<String, String>> {
    let s = extract_scenario(scenario)?;
    run_specialize(strategy, s).map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
}

fn extract_scenario(d: &Bound<'_, PyDict>) -> PyResult<Scenario> {
    let get_str = |key: &str| -> PyResult<String> {
        d.get_item(key)?
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(key.to_string()))?
            .extract::<String>()
    };
    let get_f64 = |key: &str| -> PyResult<f64> {
        d.get_item(key)?
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(key.to_string()))?
            .extract::<f64>()
    };
    let opt_str =
        |key: &str| -> PyResult<Option<String>> { d.get_item(key)?.map(|v| v.extract::<String>()).transpose() };
    let opt_f64 = |key: &str| -> PyResult<Option<f64>> { d.get_item(key)?.map(|v| v.extract::<f64>()).transpose() };
    let opt_bool = |key: &str| -> PyResult<Option<bool>> { d.get_item(key)?.map(|v| v.extract::<bool>()).transpose() };
    let opt_usize =
        |key: &str| -> PyResult<Option<usize>> { d.get_item(key)?.map(|v| v.extract::<usize>()).transpose() };
    let opt_u64 = |key: &str| -> PyResult<Option<u64>> { d.get_item(key)?.map(|v| v.extract::<u64>()).transpose() };

    let run_obj = match d.get_item("run_obj")? {
        Some(v) => match v.extract::<String>()?.to_lowercase().as_str() {
            "quality" => RunObjective::Quality,
            _ => RunObjective::Runtime,
        },
        None => RunObjective::Runtime,
    };
    let overall_obj = match d.get_item("overall_obj")? {
        Some(v) => match v.extract::<String>()?.to_lowercase().as_str() {
            "median" => OverallObjective::Median,
            _ => OverallObjective::Mean,
        },
        None => OverallObjective::Mean,
    };

    Ok(Scenario {
        algo: get_str("algo")?,
        paramfile: get_str("paramfile")?,
        initial_config: None,
        initial_config_file: None,
        instance_file: opt_str("instance_file")?,
        instances: d
            .get_item("instances")?
            .map(|v| v.extract::<Vec<String>>())
            .transpose()?,
        test_instance_file: opt_str("test_instance_file")?,
        cutoff_time: get_f64("cutoff_time")?,
        tuner_timeout: get_f64("tuner_timeout")?,
        run_obj,
        overall_obj,
        approach: opt_str("approach")?.unwrap_or_else(|| "focused".to_string()),
        perturbation_strength: opt_usize("perturbation_strength")?.unwrap_or(4),
        restart_probability: opt_f64("restart_probability")?.unwrap_or(0.0),
        restart_failures: opt_usize("restart_failures")?.unwrap_or(0),
        restart_target: opt_str("restart_target")?.unwrap_or_else(|| "incumbent".to_string()),
        restart_strength: opt_usize("restart_strength")?.unwrap_or(0),
        acceptance_tolerance: opt_f64("acceptance_tolerance")?.unwrap_or(0.0),
        random_probes: opt_usize("random_probes")?.unwrap_or(0),
        initial_fidelity: opt_usize("initial_fidelity")?.unwrap_or(1),
        fidelity_step: opt_usize("fidelity_step")?.unwrap_or(1),
        bound_multiplier: opt_f64("bound_multiplier")?.unwrap_or(10.0),
        pruning: opt_bool("pruning")?.unwrap_or(true),
        iterative_deepening: opt_bool("iterative_deepening")?.unwrap_or(false),
        lambda_n: opt_f64("lambda_n")?.unwrap_or(0.5),
        lambda_c: opt_f64("lambda_c")?.unwrap_or(0.5),
        lambda_t: opt_f64("lambda_t")?.unwrap_or(0.5),
        cores: opt_usize("cores")?.unwrap_or(0),
        num_run: opt_u64("num_run")?.unwrap_or(0),
        cache_db: opt_str("cache_db")?.unwrap_or_else(|| ":memory:".to_string()),
        debug: opt_bool("debug")?.unwrap_or(false),
        debug_wrapper: opt_bool("debug_wrapper")?.unwrap_or(false),
        debug_solver: opt_bool("debug_solver")?.unwrap_or(false),
        debug_log: opt_str("debug_log")?,
        error_log: opt_str("error_log")?,
    })
}

fn run_specialize(strategy: HashMap<String, String>, scenario: Scenario) -> anyhow::Result<HashMap<String, String>> {
    if scenario.debug {
        crate::enable_debug_stderr();
    }
    if let Some(ref path) = scenario.debug_log {
        crate::init_log_file(path)?;
    }
    if let Some(ref path) = scenario.error_log {
        crate::init_error_log(path)?;
    }

    let result = run_specialize_inner(strategy, &scenario);

    if scenario.debug_log.is_some() {
        crate::close_log_file();
    }
    if scenario.error_log.is_some() {
        crate::close_error_log();
    }

    result
}

fn run_specialize_inner(
    strategy: HashMap<String, String>,
    scenario: &Scenario,
) -> anyhow::Result<HashMap<String, String>> {
    let space = ParamSpace::from_file(&scenario.paramfile)?;
    let instance_paths = scenario.instance_paths()?;
    anyhow::ensure!(!instance_paths.is_empty(), "instance list is empty");

    let mut cache = Cache::open(&scenario.cache_db, false)?;
    let id_map = cache.load_instances(&instance_paths)?;
    let instances: Vec<(i64, String)> = instance_paths.iter().map(|p| (id_map[p], p.clone())).collect();

    let n_workers = if scenario.cores == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        scenario.cores
    };
    let debug = crate::any_debug_active();
    let options = scenario.ils_options(
        n_workers,
        crate::DebugOptions {
            main: debug,
            wrapper: scenario.debug_wrapper,
            solver: scenario.debug_solver,
        },
    )?;

    let (result, _) = if scenario.iterative_deepening {
        ils::iterative_deepening_ils(
            Some(strategy),
            &options,
            &space,
            &instances,
            &scenario.algo,
            scenario.cutoff_time,
            &mut cache,
            scenario.lambda_n,
            scenario.lambda_c,
            scenario.lambda_t,
        )?
    } else {
        ils::run(
            Some(strategy),
            &options,
            &space,
            &instances,
            &scenario.algo,
            scenario.cutoff_time,
            &mut cache,
        )?
    };

    let active = space.active_params(&result);
    Ok(active
        .iter()
        .filter_map(|p| result.get(&p.name).map(|v| (p.name.clone(), v.clone())))
        .collect())
}

#[pymodule]
fn _ramparils(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(specialize, m)?)?;
    Ok(())
}
