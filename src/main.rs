//! CLI entry point — mirrors `param_ils_2_3_run.rb` flags for drop-in compatibility.

use anyhow::Result;
use clap::Parser;

use ramparils::cache::Cache;
use ramparils::ils::{self, Approach, IlsOptions};
use ramparils::DebugOptions;
use ramparils::params::ParamSpace;
use ramparils::scenario::{self, RunObjective, OverallObjective, Scenario};

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

    /// Enable iterative deepening
    #[arg(long = "id", default_value_t = false)]
    iterative_deepening: bool,

    /// Iterative deepening: instance-count growth factor (0 < λ_n ≤ 1)
    #[arg(long = "lambda-n", default_value_t = 0.5)]
    lambda_n: f64,

    /// Iterative deepening: cutoff-time growth factor (0 < λ_c ≤ 1)
    #[arg(long = "lambda-c", default_value_t = 0.5)]
    lambda_c: f64,

    /// Iterative deepening: per-phase timeout growth factor (0 < λ_t ≤ 1)
    #[arg(long = "lambda-t", default_value_t = 0.5)]
    lambda_t: f64,

    /// Path to the result cache database (shared across runs)
    #[arg(long = "cachedb", default_value = "paramils_cache.db")]
    cache_db: String,

    /// Number of parallel worker threads (0 = all available cores)
    #[arg(long = "cores", default_value_t = 0)]
    cores: usize,

    /// Print debug output (new incumbents and their quality)
    #[arg(long = "debug", default_value_t = false)]
    debug: bool,

    /// Print every solver wrapper invocation
    #[arg(long = "debug-wrapper", default_value_t = false)]
    debug_wrapper: bool,

    /// Print every solver result
    #[arg(long = "debug-solver", default_value_t = false)]
    debug_solver: bool,

    /// Write debug output to this file (independent of --debug)
    #[arg(long = "debug-log")]
    debug_log: Option<String>,

    /// Write crash reports (failed solver runs) to this file
    #[arg(long = "error-log")]
    error_log: Option<String>,
}

fn sh(cmd: &str) -> String {
    std::process::Command::new("sh").args(["-c", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "?".to_string())
}

fn print_debug_header() {
    if !ramparils::any_debug_active() { return; }
    let t = ramparils::t();
    let d = true; // header always goes to all active destinations
    let sep = "-".repeat(60);
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] binary:  {} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")));
    ramparils::debug_line(d, &format!("[{t:8.2}s] date:    {}", sh("date")));
    ramparils::debug_line(d, &format!("[{t:8.2}s] host:    {}  ({})", sh("hostname"), sh("uname -sr")));
    ramparils::debug_line(d, &format!("[{t:8.2}s] user:    {}", std::env::var("USER").unwrap_or_else(|_| sh("whoami"))));
    ramparils::debug_line(d, &format!("[{t:8.2}s] pid:     {}", std::process::id()));
    ramparils::debug_line(d, &format!("[{t:8.2}s] cwd:     {}", std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".to_string())));
    let argv: Vec<String> = std::env::args().collect();
    ramparils::debug_line(d, &format!("[{t:8.2}s] args:    {}", argv.join(" ")));
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
}

fn print_debug_scenario(s: &Scenario, n_instances: usize, n_workers: usize, approach: &str) {
    if !ramparils::any_debug_active() { return; }
    let t = ramparils::t();
    let d = true;
    let sep = "-".repeat(60);
    let run_obj = match s.run_obj { RunObjective::Runtime => "runtime", RunObjective::Quality => "quality" };
    let overall_obj = match s.overall_obj { OverallObjective::Mean => "mean", OverallObjective::Median => "median" };
    let test = s.test_instance_file.as_deref().unwrap_or("-");
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] algo:       {}", s.algo));
    ramparils::debug_line(d, &format!("[{t:8.2}s] paramfile:  {}", s.paramfile));
    ramparils::debug_line(d, &format!("[{t:8.2}s] instances:  {} ({n_instances} loaded)", s.instance_file));
    ramparils::debug_line(d, &format!("[{t:8.2}s] test:       {test}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] cutoff:     {}s", s.cutoff_time));
    ramparils::debug_line(d, &format!("[{t:8.2}s] timeout:    {}s", s.tuner_timeout));
    ramparils::debug_line(d, &format!("[{t:8.2}s] objective:  {run_obj} / {overall_obj}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] approach:   {approach}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] workers:    {n_workers}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.debug { ramparils::enable_debug_stderr(); }
    if let Some(ref path) = args.debug_log { ramparils::init_log_file(path)?; }
    if let Some(ref path) = args.error_log { ramparils::init_error_log(path)?; }
    // Main debug is active when either output destination is configured.
    let main_debug = ramparils::any_debug_active();
    print_debug_header();

    // Load scenario and parameter space
    let scenario = Scenario::from_file(&args.scenariofile)?;
    let space = ParamSpace::from_file(&scenario.paramfile)?;

    // Load and register instances
    let instance_paths = scenario::load_instances(&scenario.instance_file)?;
    anyhow::ensure!(!instance_paths.is_empty(), "instance file is empty: {}", scenario.instance_file);

    let mut cache = Cache::open(&args.cache_db, main_debug)?;
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
        debug: DebugOptions { main: main_debug, wrapper: args.debug_wrapper, solver: args.debug_solver },
    };

    print_debug_scenario(&scenario, instances.len(), n_workers, &args.approach);

    // Start from the parameter space's default configuration
    let initial = space.default_config();

    // Run ILS
    let (result, best_score) = if args.iterative_deepening {
        ils::iterative_deepening_ils(
            Some(initial),
            &options,
            &space,
            &instances,
            &scenario.algo,
            scenario.cutoff_time,
            &mut cache,
            args.lambda_n,
            args.lambda_c,
            args.lambda_t,
        )?
    } else {
        ils::run(
            Some(initial),
            &options,
            &space,
            &instances,
            &scenario.algo,
            scenario.cutoff_time,
            &mut cache,
        )?
    };

    ramparils::debug_line(main_debug, &format!("[{:8.2}s] ils: best score: {best_score:.6}", ramparils::t()));

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
