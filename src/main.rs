//! CLI entry point.

use anyhow::Result;
use clap::Parser;

use ramparils::cache::Cache;
use ramparils::ils::{self, Approach, IlsOptions};
use ramparils::params::ParamSpace;
use ramparils::scenario::{RunObjective, OverallObjective, Scenario};
use ramparils::DebugOptions;

#[derive(Parser, Debug)]
#[command(name = "ramparils", about = "Automated algorithm configuration via ILS")]
struct Args {
    /// Scenario file (YAML): defines algo, instances, cutoff, tuner knobs, …
    #[arg(long)]
    scenariofile: String,
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
    let d = true;
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

fn print_debug_scenario(s: &Scenario, n_instances: usize, n_workers: usize) {
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
    ramparils::debug_line(d, &format!("[{t:8.2}s] instances:  {} ({n_instances} loaded)", s.instance_source_label()));
    ramparils::debug_line(d, &format!("[{t:8.2}s] test:       {test}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] cutoff:     {}s", s.cutoff_time));
    ramparils::debug_line(d, &format!("[{t:8.2}s] timeout:    {}s", s.tuner_timeout));
    ramparils::debug_line(d, &format!("[{t:8.2}s] objective:  {run_obj} / {overall_obj}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] approach:   {}", s.approach));
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] fidelity:   initial={} step={}",
            s.initial_fidelity, s.fidelity_step
        ),
    );
    ramparils::debug_line(d, &format!("[{t:8.2}s] workers:    {n_workers}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
}

fn main() -> Result<()> {
    ramparils::install_signal_handlers()?;
    let args = Args::parse();

    let scenario = Scenario::from_file(&args.scenariofile)?;

    if scenario.debug { ramparils::enable_debug_stderr(); }
    if let Some(ref path) = scenario.debug_log { ramparils::init_log_file(path)?; }
    if let Some(ref path) = scenario.error_log { ramparils::init_error_log(path)?; }
    let main_debug = ramparils::any_debug_active();
    print_debug_header();

    let space = ParamSpace::from_file(&scenario.paramfile)?;

    let instance_paths = scenario.instance_paths()?;
    anyhow::ensure!(!instance_paths.is_empty(), "instance list is empty");

    let mut cache = Cache::open(&scenario.cache_db, main_debug)?;
    let id_map = cache.load_instances(&instance_paths)?;
    let instances: Vec<(i64, String)> = instance_paths.iter()
        .map(|p| (id_map[p], p.clone()))
        .collect();

    let approach = match scenario.approach.to_lowercase().as_str() {
        "basic"  => Approach::Basic,
        "random" => Approach::Random,
        _        => Approach::Focused,
    };
    let n_workers = if scenario.cores == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        scenario.cores
    };
    let options = IlsOptions {
        approach,
        n_workers,
        perturbation_strength: scenario.perturbation_strength,
        initial_fidelity: scenario.initial_fidelity,
        fidelity_step: scenario.fidelity_step,
        bound_multiplier: scenario.bound_multiplier,
        pruning: scenario.pruning,
        tuner_timeout: scenario.tuner_timeout,
        run_obj: scenario.run_obj.clone(),
        overall_obj: scenario.overall_obj.clone(),
        debug: DebugOptions {
            main: main_debug,
            wrapper: scenario.debug_wrapper,
            solver: scenario.debug_solver,
        },
    };

    print_debug_scenario(&scenario, instances.len(), n_workers);

    let initial = space.default_config();

    let (result, best_score) = if scenario.iterative_deepening {
        ils::iterative_deepening_ils(
            Some(initial),
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
            Some(initial),
            &options,
            &space,
            &instances,
            &scenario.algo,
            scenario.cutoff_time,
            &mut cache,
        )?
    };
    if ramparils::interrupted() {
        ramparils::terminate_active_process_groups();
        std::process::exit(130);
    }

    ramparils::debug_line(main_debug, &format!("[{:8.2}s] ils: best score: {best_score:.6}", ramparils::t()));

    let active = space.active_params(&result);
    let mut pairs: Vec<(&String, &String)> = active.iter()
        .filter_map(|p| result.get(&p.name).map(|v| (&p.name, v)))
        .collect();
    pairs.sort_by_key(|(k, _)| k.as_str());
    let param_str = pairs.iter().map(|(k, v)| format!("-{k} {v}")).collect::<Vec<_>>().join(" ");
    println!("{param_str}");

    Ok(())
}
