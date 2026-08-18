//! CLI entry point.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use ramparils::DebugOptions;
use ramparils::cache::Cache;
use ramparils::ils;
use ramparils::params::{ParamSpace, config_to_yaml};
use ramparils::scenario::{OverallObjective, RunObjective, Scenario};

#[derive(Parser, Debug)]
#[command(
    name = "ramparils",
    about = "Automated algorithm configuration via ILS",
    version,
    long_version = concat!(
        env!("CARGO_PKG_VERSION"), "\n",
        "git:   ", env!("RAMPARILS_GIT"), "\n",
        "build: ", env!("RAMPARILS_BUILD"),
    ),
)]
struct Args {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Tune a target algorithm from a scenario file.
    Run {
        /// Scenario file (YAML): defines algo, instances, cutoff, tuner knobs, …
        scenario: String,
    },

    /// Export the contents of a `.dbcache` to files.
    #[command(subcommand)]
    Db(DbCommand),
}

/// Sub-commands of `ramparils db`.
///
/// All three write one file per strategy hash, named `ram-<hash>`, under a
/// layout that mirrors solverpy's database so an export can be dropped into an
/// existing `solverpy_db/`. They report a one-line summary on stdout; stderr
/// carries errors only.
#[derive(Subcommand, Debug)]
enum DbCommand {
    /// Write the instances each strategy solved, one path per line.
    ///
    /// A result counts as solved when its status is one of `Theorem`,
    /// `Unsatisfiable`, `Satisfiable`, `CounterSatisfiable`,
    /// `ContradictoryAxioms`, `sat`, `unsat` — the union of TPTP's and
    /// SMT-LIB's success tokens, as solverpy uses. RamParILS stores the status
    /// verbatim and never interprets it when scoring, so a target algorithm
    /// reporting anything else is not recognised here and its instances will
    /// be reported as unsolved.
    Solved {
        /// Path to the .dbcache file.
        dbcache: PathBuf,

        /// Output root directory.
        #[arg(long, default_value = "solverpy_db")]
        out_dir: PathBuf,
    },

    /// Write every result as `instance <TAB> status <TAB> runtime`.
    Status {
        /// Path to the .dbcache file.
        dbcache: PathBuf,

        /// Output root directory.
        #[arg(long, default_value = "solverpy_db")]
        out_dir: PathBuf,
    },

    /// Write the configuration behind each strategy hash, as YAML.
    ///
    /// A conf file is a parameter assignment, so it only means anything
    /// against the parameter space it was tuned in — unlike solverpy's
    /// `strats/`, which holds solver command lines.
    ///
    /// It records the ACTIVE configuration: parameters whose guard was closed
    /// are absent, because that is what the cache keys on. A conf file is
    /// therefore a record of what ran rather than a complete configuration,
    /// and `initial_config_file` will reject it unless every parameter in the
    /// space happened to be active.
    Confs {
        /// Path to the .dbcache file.
        dbcache: PathBuf,

        /// Output root directory.
        #[arg(long, default_value = "solverpy_db")]
        out_dir: PathBuf,

        /// Write the stored JSON instead of YAML.
        #[arg(long)]
        json: bool,
    },
}

fn sh(cmd: &str) -> String {
    std::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "?".to_string())
}

fn print_debug_header() {
    if !ramparils::any_debug_active() {
        return;
    }
    let t = ramparils::t();
    let d = true;
    let sep = "-".repeat(60);
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] binary:  {} v{}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ),
    );
    ramparils::debug_line(d, &format!("[{t:8.2}s] git:     {}", ramparils::GIT_REVISION));
    ramparils::debug_line(d, &format!("[{t:8.2}s] build:   {}", ramparils::BUILD_INFO));
    ramparils::debug_line(d, &format!("[{t:8.2}s] date:    {}", sh("date")));
    ramparils::debug_line(
        d,
        &format!("[{t:8.2}s] host:    {}  ({})", sh("hostname"), sh("uname -sr")),
    );
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] user:    {}",
            std::env::var("USER").unwrap_or_else(|_| sh("whoami"))
        ),
    );
    ramparils::debug_line(d, &format!("[{t:8.2}s] pid:     {}", std::process::id()));
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] cwd:     {}",
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".to_string())
        ),
    );
    let argv: Vec<String> = std::env::args().collect();
    ramparils::debug_line(d, &format!("[{t:8.2}s] args:    {}", argv.join(" ")));
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
}

fn print_debug_scenario(s: &Scenario, n_instances: usize, n_workers: usize) {
    if !ramparils::any_debug_active() {
        return;
    }
    let t = ramparils::t();
    let d = true;
    let sep = "-".repeat(60);
    let run_obj = match s.run_obj {
        RunObjective::Runtime => "runtime",
        RunObjective::Quality => "quality",
    };
    let overall_obj = match s.overall_obj {
        OverallObjective::Mean => "mean",
        OverallObjective::Median => "median",
    };
    let test = s.test_instance_file.as_deref().unwrap_or("-");
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] algo:       {}", s.algo));
    ramparils::debug_line(d, &format!("[{t:8.2}s] paramfile:  {}", s.paramfile));
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] instances:  {} ({n_instances} loaded)",
            s.instance_source_label()
        ),
    );
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
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] perturb:    strength={} restart_strength={}",
            s.perturbation_strength,
            if s.restart_strength == 0 {
                2 * s.perturbation_strength
            } else {
                s.restart_strength
            },
        ),
    );
    ramparils::debug_line(
        d,
        &format!(
            "[{t:8.2}s] restart:    p={} failures={} target={} tolerance={} probes={}",
            s.restart_probability, s.restart_failures, s.restart_target, s.acceptance_tolerance, s.random_probes,
        ),
    );
    ramparils::debug_line(d, &format!("[{t:8.2}s] workers:    {n_workers}"));
    ramparils::debug_line(d, &format!("[{t:8.2}s] {sep}"));
}

fn main() -> Result<()> {
    match Args::parse().cmd {
        Command::Run { scenario } => {
            ramparils::install_signal_handlers()?;
            cmd_run(&scenario)
        }
        Command::Db(DbCommand::Solved { dbcache, out_dir }) => ramparils::db::solved(&dbcache, &out_dir),
        Command::Db(DbCommand::Status { dbcache, out_dir }) => ramparils::db::status(&dbcache, &out_dir),
        Command::Db(DbCommand::Confs { dbcache, out_dir, json }) => ramparils::db::confs(&dbcache, &out_dir, json),
    }
}

/// `ramparils run <scenario>`: the tuning loop.
fn cmd_run(scenariofile: &str) -> Result<()> {
    let scenario = Scenario::from_file(scenariofile)?;

    if scenario.debug {
        ramparils::enable_debug_stderr();
    }
    if let Some(ref path) = scenario.debug_log {
        ramparils::init_log_file(path)?;
    }
    if let Some(ref path) = scenario.error_log {
        ramparils::init_error_log(path)?;
    }
    let main_debug = ramparils::any_debug_active();
    print_debug_header();

    let space = ParamSpace::from_file(&scenario.paramfile)?;

    let instance_paths = scenario.instance_paths()?;
    anyhow::ensure!(!instance_paths.is_empty(), "instance list is empty");

    let mut cache = Cache::open(&scenario.cache_db, main_debug)?;
    let id_map = cache.load_instances(&instance_paths)?;
    let instances: Vec<(i64, String)> = instance_paths.iter().map(|p| (id_map[p], p.clone())).collect();

    let n_workers = if scenario.cores == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        scenario.cores
    };
    let options = scenario.ils_options(
        n_workers,
        DebugOptions {
            main: main_debug,
            wrapper: scenario.debug_wrapper,
            solver: scenario.debug_solver,
        },
    )?;

    print_debug_scenario(&scenario, instances.len(), n_workers);

    let initial = scenario.resolve_initial_config(&space)?;

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

    let result_yaml = config_to_yaml(&result)?;
    ramparils::debug_line(
        main_debug,
        &format!("[{:8.2}s] ils: final config: score={best_score:.6}", ramparils::t()),
    );
    ramparils::debug_block(main_debug, &result_yaml);
    print!("{result_yaml}");

    Ok(())
}
