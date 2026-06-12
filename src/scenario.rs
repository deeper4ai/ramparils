//! Scenario: defines what to tune and how.
//!
//! From the CLI, a scenario is loaded from a YAML file.  The only required
//! keys are `algo`, `paramfile`, one of `instance_file` / `instances`,
//! `cutoff_time`, and `tuner_timeout`.  Everything else has a sensible default.
//!
//! ```yaml
//! algo: path/to/solver_wrapper
//! paramfile: solver.params
//! initial_config:             # optional; must contain every parameter
//!   engine: quick
//!   threads: 4
//! # initial_config_file: initial-config.yaml  # alternative YAML mapping
//! instance_file: instances/train.txt
//! test_instance_file: instances/test.txt   # optional
//! cutoff_time: 60.0
//! tuner_timeout: 300.0
//! # --- tuner knobs (all optional, shown with defaults) ---
//! run_obj: runtime            # runtime | quality
//! overall_obj: mean           # mean | median
//! approach: focused           # focused | basic | random
//! perturbation_strength: 4
//! initial_fidelity: 1
//! fidelity_step: 1
//! bound_multiplier: 10.0
//! pruning: true
//! iterative_deepening: false
//! lambda_n: 0.5
//! lambda_c: 0.5
//! lambda_t: 0.5
//! cores: 0                    # 0 = all available
//! num_run: 0
//! cache_db: ":memory:"          # use a file path to persist across runs
//! debug: false
//! debug_wrapper: false
//! debug_solver: false
//! debug_log: ~                # path, or omit / null
//! ```
//!
//! From Python, a scenario is passed as a dict — no file needed.
//! `instances` (a list of paths) may be used instead of `instance_file`.

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer};
use std::{collections::HashMap, fs};

use crate::params::{Config, ParamSpace};

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

fn default_approach() -> String { "focused".to_string() }
fn default_perturbation_strength() -> usize { 4 }
fn default_fidelity() -> usize { 1 }
fn default_bound_multiplier() -> f64 { 10.0 }
fn default_pruning() -> bool { true }
fn default_lambda() -> f64 { 0.5 }
fn default_cache_db() -> String { ":memory:".to_string() }
fn default_run_obj() -> RunObjective { RunObjective::Runtime }
fn default_overall_obj() -> OverallObjective { OverallObjective::Mean }

/// Full scenario description — the single source of truth for all tuning knobs.
#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    /// Command (or path) used to run the target algorithm.
    pub algo: String,

    /// Path to the `.params` file describing the parameter space.
    pub paramfile: String,

    /// Complete initial parameter configuration, as an inline YAML mapping.
    #[serde(default, deserialize_with = "deserialize_optional_config")]
    pub initial_config: Option<Config>,

    /// YAML file containing the complete initial parameter configuration.
    #[serde(default)]
    pub initial_config_file: Option<String>,

    /// File listing training instance paths, one per line.
    /// Mutually exclusive with `instances`; one must be set.
    #[serde(default)]
    pub instance_file: Option<String>,

    /// Inline list of training instance paths (Python / programmatic use).
    /// Mutually exclusive with `instance_file`.
    #[serde(default)]
    pub instances: Option<Vec<String>>,

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

    /// ILS variant: `"focused"`, `"basic"`, or `"random"`.
    #[serde(default = "default_approach")]
    pub approach: String,

    /// Neighbourhood steps per perturbation.
    #[serde(default = "default_perturbation_strength")]
    pub perturbation_strength: usize,

    /// Initial number of instances used to evaluate each configuration.
    #[serde(default = "default_fidelity")]
    pub initial_fidelity: usize,

    /// Number of instances added when FocusedILS increases evaluation fidelity.
    #[serde(default = "default_fidelity")]
    pub fidelity_step: usize,

    /// Adaptive capping multiplier.
    #[serde(default = "default_bound_multiplier")]
    pub bound_multiplier: f64,

    /// Enable adaptive capping / pruning.
    #[serde(default = "default_pruning")]
    pub pruning: bool,

    /// Enable iterative deepening.
    #[serde(default)]
    pub iterative_deepening: bool,

    /// Iterative deepening: instance-count growth factor (0 < λ_n ≤ 1).
    #[serde(default = "default_lambda")]
    pub lambda_n: f64,

    /// Iterative deepening: cutoff-time growth factor (0 < λ_c ≤ 1).
    #[serde(default = "default_lambda")]
    pub lambda_c: f64,

    /// Iterative deepening: per-phase timeout growth factor (0 < λ_t ≤ 1).
    #[serde(default = "default_lambda")]
    pub lambda_t: f64,

    /// Parallel worker threads (0 = all available cores).
    #[serde(default)]
    pub cores: usize,

    /// Run index / random seed (reserved for future use).
    #[serde(default)]
    pub num_run: u64,

    /// Path to the SQLite result cache.
    #[serde(default = "default_cache_db")]
    pub cache_db: String,

    /// Print debug output (new incumbents and their quality).
    #[serde(default)]
    pub debug: bool,

    /// Print every solver wrapper invocation.
    #[serde(default)]
    pub debug_wrapper: bool,

    /// Print every solver result.
    #[serde(default)]
    pub debug_solver: bool,

    /// Write debug output to this file (independent of `debug`).
    #[serde(default)]
    pub debug_log: Option<String>,

    /// Write crash reports (failed solver runs) to this file.
    #[serde(default)]
    pub error_log: Option<String>,
}

impl Scenario {
    /// Load a scenario from a YAML file.
    pub fn from_file(path: &str) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("Cannot read scenario file: {path}"))?;
        serde_yaml::from_str(&text)
            .with_context(|| format!("Failed to parse scenario YAML: {path}"))
    }

    /// Resolve the instance list from either `instances` or `instance_file`.
    pub fn instance_paths(&self) -> Result<Vec<String>> {
        if let Some(ref list) = self.instances {
            return Ok(list.clone());
        }
        let file = self.instance_file.as_deref()
            .ok_or_else(|| anyhow::anyhow!("scenario: neither 'instance_file' nor 'instances' is set"))?;
        load_instances(file)
    }

    /// Human-readable label for the instance source (for debug output).
    pub fn instance_source_label(&self) -> String {
        if self.instances.is_some() {
            "<inline list>".to_string()
        } else {
            self.instance_file.clone().unwrap_or_else(|| "<none>".to_string())
        }
    }

    /// Resolve and validate the ILS starting configuration.
    ///
    /// Explicit configurations must contain every parameter. If neither form
    /// is supplied, parameter-file defaults preserve the historical behavior.
    pub fn resolve_initial_config(&self, space: &ParamSpace) -> Result<Config> {
        anyhow::ensure!(
            self.initial_config.is_none() || self.initial_config_file.is_none(),
            "scenario: 'initial_config' and 'initial_config_file' are mutually exclusive"
        );

        let config = if let Some(config) = &self.initial_config {
            config.clone()
        } else if let Some(path) = &self.initial_config_file {
            let text = fs::read_to_string(path)
                .with_context(|| format!("Cannot read initial configuration file: {path}"))?;
            parse_config_yaml(&text)
                .with_context(|| format!("Failed to parse initial configuration YAML: {path}"))?
        } else {
            space.default_config()
        };

        space.validate_config(&config)?;
        Ok(config)
    }
}

fn deserialize_optional_config<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Config>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_yaml::Value>::deserialize(deserializer)?;
    value
        .map(config_from_yaml_value)
        .transpose()
        .map_err(serde::de::Error::custom)
}

fn parse_config_yaml(text: &str) -> Result<Config> {
    let value: serde_yaml::Value = serde_yaml::from_str(text)?;
    config_from_yaml_value(value)
}

fn config_from_yaml_value(value: serde_yaml::Value) -> Result<Config> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("initial configuration must be a YAML mapping"))?;
    let mut config = HashMap::with_capacity(mapping.len());

    for (key, value) in mapping {
        let name = key.as_str().ok_or_else(|| {
            anyhow::anyhow!("initial configuration parameter names must be strings")
        })?;
        let value = match value {
            serde_yaml::Value::String(value) => value.clone(),
            serde_yaml::Value::Bool(value) => value.to_string(),
            serde_yaml::Value::Number(value) => value.to_string(),
            _ => anyhow::bail!(
                "initial configuration value for '{name}' must be a string, number, or boolean"
            ),
        };
        config.insert(name.to_string(), value);
    }

    Ok(config)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn space() -> ParamSpace {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "mode {{fast, slow}} [fast]").unwrap();
        writeln!(file, "limit {{1, 2}} [1]").unwrap();
        ParamSpace::from_file(file.path().to_str().unwrap()).unwrap()
    }

    fn scenario(extra: &str) -> Scenario {
        serde_yaml::from_str(&format!(
            "algo: solver\nparamfile: params\ninstances: [one]\ncutoff_time: 1\ntuner_timeout: 2\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn resolves_inline_initial_config_with_scalar_values() {
        let scenario = scenario("initial_config:\n  mode: slow\n  limit: 2\n");
        let config = scenario.resolve_initial_config(&space()).unwrap();
        assert_eq!(config["mode"], "slow");
        assert_eq!(config["limit"], "2");
    }

    #[test]
    fn rejects_both_initial_config_forms() {
        let scenario = scenario(
            "initial_config:\n  mode: slow\n  limit: 2\ninitial_config_file: initial.yaml\n",
        );
        assert!(
            scenario
                .resolve_initial_config(&space())
                .unwrap_err()
                .to_string()
                .contains("mutually exclusive")
        );
    }

    #[test]
    fn resolves_initial_config_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "mode: slow\nlimit: 2").unwrap();
        let scenario = scenario(&format!(
            "initial_config_file: {:?}\n",
            file.path().to_str().unwrap()
        ));
        let config = scenario.resolve_initial_config(&space()).unwrap();
        assert_eq!(config["mode"], "slow");
        assert_eq!(config["limit"], "2");
    }

    #[test]
    fn falls_back_to_parameter_defaults() {
        let config = scenario("").resolve_initial_config(&space()).unwrap();
        assert_eq!(config["mode"], "fast");
        assert_eq!(config["limit"], "1");
    }
}
