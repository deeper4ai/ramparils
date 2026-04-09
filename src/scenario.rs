//! Scenario: defines what to tune and how.
//!
//! From the CLI, a scenario is loaded from a YAML file:
//!
//! ```yaml
//! algo: path/to/solver_wrapper
//! paramfile: solver.params
//! instance_file: instances/train.txt
//! test_instance_file: instances/test.txt   # optional
//! cutoff_time: 60.0
//! tuner_timeout: 300.0
//! run_obj: runtime    # runtime | quality
//! overall_obj: mean   # mean | median
//! ```
//!
//! From Python, a scenario is passed as a dict — no file needed.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunObjective {
    Runtime,
    Quality,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallObjective {
    Mean,
    Median,
}

/// Full scenario description.
///
/// Derived traits:
/// - `Deserialize` — for YAML file loading
/// - `FromPyObject` (feature = "python") — for Python dict passing
#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    /// Command (or path) used to run the target algorithm.
    pub algo: String,

    /// Path to the `.params` file describing the parameter space.
    pub paramfile: String,

    /// File listing training instance paths, one per line.
    pub instance_file: String,

    /// Optional file listing test instance paths (for final evaluation).
    #[serde(default)]
    pub test_instance_file: Option<String>,

    /// Per-run time limit in seconds (passed to the target algorithm).
    pub cutoff_time: f64,

    /// Total wall-clock budget for the tuner in seconds.
    pub tuner_timeout: f64,

    /// What a single run optimizes: `runtime` or `quality`.
    #[serde(default = "default_run_obj")]
    pub run_obj: RunObjective,

    /// How per-run results are aggregated: `mean` or `median`.
    #[serde(default = "default_overall_obj")]
    pub overall_obj: OverallObjective,
}

fn default_run_obj() -> RunObjective {
    RunObjective::Runtime
}

fn default_overall_obj() -> OverallObjective {
    OverallObjective::Mean
}

#[cfg(feature = "python")]
impl pyo3::FromPyObject<'_> for RunObjective {
    fn extract_bound(ob: &pyo3::Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<Self> {
        use pyo3::prelude::PyAnyMethods;
        match ob.extract::<String>()?.to_lowercase().as_str() {
            "runtime" => Ok(RunObjective::Runtime),
            "quality" => Ok(RunObjective::Quality),
            s => Err(pyo3::exceptions::PyValueError::new_err(
                format!("unknown run_obj '{s}': expected 'runtime' or 'quality'"),
            )),
        }
    }
}

#[cfg(feature = "python")]
impl pyo3::FromPyObject<'_> for OverallObjective {
    fn extract_bound(ob: &pyo3::Bound<'_, pyo3::PyAny>) -> pyo3::PyResult<Self> {
        use pyo3::prelude::PyAnyMethods;
        match ob.extract::<String>()?.to_lowercase().as_str() {
            "mean"   => Ok(OverallObjective::Mean),
            "median" => Ok(OverallObjective::Median),
            s => Err(pyo3::exceptions::PyValueError::new_err(
                format!("unknown overall_obj '{s}': expected 'mean' or 'median'"),
            )),
        }
    }
}

impl Scenario {
    /// Load a scenario from a YAML file.
    pub fn from_file(path: &str) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("Cannot read scenario file: {path}"))?;
        serde_yaml::from_str(&text)
            .with_context(|| format!("Failed to parse scenario YAML: {path}"))
    }
}

/// Read instance paths from a file: one path per line, blank lines and
/// lines starting with `#` are ignored.
pub fn load_instances(path: &str) -> Result<Vec<String>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Cannot read instance file: {path}"))?;
    Ok(text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect())
}
