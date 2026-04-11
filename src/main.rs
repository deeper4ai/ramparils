//! CLI entry point — mirrors `param_ils_2_3_run.rb` flags for drop-in compatibility.

use anyhow::Result;
use clap::Parser;

use ramparils::cache::Cache;
use ramparils::ils::{self, Approach, IlsOptions};
use ramparils::params::ParamSpace;
use ramparils::scenario::{self, Scenario};

#[derive(Parser, Debug)]
#[command(name = "ramparils", about = "Automated algorithm configuration via ILS")]
struct Args {
    /// Scenario file (defines algo, paramfile, instances, cutoff_time, …)
    #[arg(long)]
    scenariofile: String,

    /// Run index (reserved for future use as a random seed)
    #[arg(long = "numRun", default_value_t = 0)]
    num_run: u64,

    /// ILS approach: basic | focused | random
    #[arg(long = "approach", default_value = "focused")]
    approach: String,

    /// Perturbation strength (neighbourhood steps per perturbation)
    #[arg(long = "ps", default_value_t = 4)]
    perturbation_strength: usize,

    /// Bound multiplier for adaptive capping
    #[arg(long = "bm", default_value_t = 10.0)]
    bound_multiplier: f64,

    /// Enable adaptive capping / pruning
    #[arg(long = "pruning", default_value_t = true)]
    pruning: bool,

    /// Enable iterative deepening (not yet implemented)
    #[arg(long = "id", default_value_t = false)]
    iterative_deepening: bool,

    /// Path to the result cache database (shared across runs)
    #[arg(long = "cachedb", default_value = "paramils_cache.db")]
    cache_db: String,

    /// Number of parallel worker threads (0 = all available cores)
    #[arg(long = "cores", default_value_t = 0)]
    cores: usize,

    /// Print debug output (new incumbents and their quality)
    #[arg(long = "debug", default_value_t = false)]
    debug: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Load scenario and parameter space
    let scenario = Scenario::from_file(&args.scenariofile)?;
    let space = ParamSpace::from_file(&scenario.paramfile)?;

    // Load and register instances
    let instance_paths = scenario::load_instances(&scenario.instance_file)?;
    anyhow::ensure!(!instance_paths.is_empty(), "instance file is empty: {}", scenario.instance_file);

    let mut cache = Cache::open(&args.cache_db)?;
    let id_map = cache.load_instances(&instance_paths)?;
    let instances: Vec<(i64, String)> = instance_paths.iter()
        .map(|p| (id_map[p], p.clone()))
        .collect();

    // Build ILS options
    let approach = match args.approach.to_lowercase().as_str() {
        "basic"  => Approach::Basic,
        "random" => Approach::Random,
        _        => Approach::Focused,
    };
    let n_workers = if args.cores == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        args.cores
    };
    let options = IlsOptions {
        approach,
        n_workers,
        perturbation_strength: args.perturbation_strength,
        bound_multiplier: args.bound_multiplier,
        pruning: args.pruning,
        tuner_timeout: scenario.tuner_timeout,
        run_obj: scenario.run_obj.clone(),
        overall_obj: scenario.overall_obj.clone(),
        debug: args.debug,
    };

    // Start from the parameter space's default configuration
    let initial = space.default_config();

    // Run ILS
    let result = ils::run(
        Some(initial),
        options,
        &space,
        &instances,
        &scenario.algo,
        scenario.cutoff_time,
        &mut cache,
    )?;

    // Print result: only active params, sorted, in -key value format
    // (compatible with Grackle's strategy parsing)
    let active = space.active_params(&result);
    let mut pairs: Vec<(&String, &String)> = active.iter()
        .filter_map(|p| result.get(&p.name).map(|v| (&p.name, v)))
        .collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    let param_str = pairs.iter().map(|(k, v)| format!("-{k} {v}")).collect::<Vec<_>>().join(" ");
    println!("{param_str}");

    Ok(())
}
