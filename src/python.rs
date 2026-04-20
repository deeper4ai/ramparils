//! PyO3 bindings — only compiled with `--features python` (used by maturin).

use std::collections::HashMap;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods};

use crate::cache::Cache;
use crate::ils::{self, Approach, IlsOptions};
use crate::params::ParamSpace;
use crate::scenario::{self, OverallObjective, RunObjective, Scenario};

/// Specialize `strategy` on the instance set defined in `scenario`.
///
/// `scenario` is a Python dict with keys:
///   algo, paramfile, instance_file, cutoff_time, tuner_timeout,
///   test_instance_file (optional), run_obj (optional), overall_obj (optional)
///
/// Returns the improved strategy as a `dict[str, str]`.
#[pyfunction]
#[pyo3(signature = (strategy, scenario, cache_db, cores=0, debug_log=None, error_log=None))]
fn specialize(
    strategy: HashMap<String, String>,
    scenario: &Bound<'_, PyDict>,
    cache_db: String,
    cores: usize,
    debug_log: Option<String>,
    error_log: Option<String>,
) -> PyResult<HashMap<String, String>> {
    let s = extract_scenario(scenario)?;
    // Accept either `instances=[...]` (list of paths) or `instance_file="..."`.
    let instance_override: Option<Vec<String>> = scenario
        .get_item("instances")?
        .map(|v| v.extract::<Vec<String>>())
        .transpose()?;
    run_specialize(strategy, s, instance_override, cache_db, cores, debug_log, error_log)
        .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
}

/// Extract a `Scenario` from a Python dict, with sensible defaults for
/// optional fields.
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

    let run_obj = match d.get_item("run_obj")? {
        Some(v) => match v.extract::<String>()?.to_lowercase().as_str() {
            "quality" => RunObjective::Quality,
            _         => RunObjective::Runtime,
        },
        None => RunObjective::Runtime,
    };
    let overall_obj = match d.get_item("overall_obj")? {
        Some(v) => match v.extract::<String>()?.to_lowercase().as_str() {
            "median" => OverallObjective::Median,
            _        => OverallObjective::Mean,
        },
        None => OverallObjective::Mean,
    };
    let test_instance_file = d.get_item("test_instance_file")?
        .map(|v| v.extract::<String>())
        .transpose()?;

    Ok(Scenario {
        algo: get_str("algo")?,
        paramfile: get_str("paramfile")?,
        // instance_file is optional when `instances` list is provided directly
        instance_file: d.get_item("instance_file")?
            .map(|v| v.extract::<String>())
            .transpose()?
            .unwrap_or_default(),
        test_instance_file,
        cutoff_time: get_f64("cutoff_time")?,
        tuner_timeout: get_f64("tuner_timeout")?,
        run_obj,
        overall_obj,
    })
}

fn run_specialize(
    strategy: HashMap<String, String>,
    scenario: Scenario,
    instance_override: Option<Vec<String>>,
    cache_db: String,
    cores: usize,
    debug_log: Option<String>,
    error_log: Option<String>,
) -> anyhow::Result<HashMap<String, String>> {
    if let Some(ref path) = debug_log { crate::init_log_file(path)?; }
    if let Some(ref path) = error_log { crate::init_error_log(path)?; }

    let result = run_specialize_inner(strategy, scenario, instance_override, cache_db, cores);

    if debug_log.is_some() { crate::close_log_file(); }
    if error_log.is_some() { crate::close_error_log(); }

    result
}

fn run_specialize_inner(
    strategy: HashMap<String, String>,
    scenario: Scenario,
    instance_override: Option<Vec<String>>,
    cache_db: String,
    cores: usize,
) -> anyhow::Result<HashMap<String, String>> {
    let space = ParamSpace::from_file(&scenario.paramfile)?;
    let instance_paths = match instance_override {
        Some(list) => list,
        None => scenario::load_instances(&scenario.instance_file)?,
    };
    anyhow::ensure!(!instance_paths.is_empty(), "instance list is empty");

    let mut cache = Cache::open(&cache_db, false)?;
    let id_map = cache.load_instances(&instance_paths)?;
    let instances: Vec<(i64, String)> = instance_paths.iter()
        .map(|p| (id_map[p], p.clone()))
        .collect();

    let n_workers = if cores == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        cores
    };
    let options = IlsOptions {
        approach: Approach::Focused,
        n_workers,
        perturbation_strength: 4,
        bound_multiplier: 10.0,
        pruning: true,
        tuner_timeout: scenario.tuner_timeout,
        run_obj: scenario.run_obj.clone(),
        overall_obj: scenario.overall_obj.clone(),
        debug: crate::DebugOptions { main: crate::any_debug_active(), wrapper: false, solver: false },
    };

    let (result, _) = ils::run(
        Some(strategy),
        &options,
        &space,
        &instances,
        &scenario.algo,
        scenario.cutoff_time,
        &mut cache,
    )?;

    let active = space.active_params(&result);
    Ok(active.iter()
        .filter_map(|p| result.get(&p.name).map(|v| (p.name.clone(), v.clone())))
        .collect())
}

#[pymodule]
fn _ramparils(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(specialize, m)?)?;
    Ok(())
}
