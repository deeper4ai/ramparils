//! `ramparils-db` — offline inspection of `.dbcache` files.
//!
//! # Sub-commands
//!
//! ## `solved`
//!
//! ```text
//! ramparils-db solved <dbcache> [--out-dir <dir>]
//! ```
//!
//! For each strategy hash present in the cache, writes a file
//! `<out-dir>/solved/<dbcache-stem>/ram-<hash>` containing the sorted list
//! of instance basenames solved by that strategy.
//!
//! "Solved" means the stored status is one of the known success tokens:
//! `Theorem`, `Unsatisfiable`, `Satisfiable`, `CounterSatisfiable`,
//! `ContradictoryAxioms` (TPTP) or `sat`, `unsat` (SMT-LIB2).
//!
//! ## `status`
//!
//! ```text
//! ramparils-db status <dbcache> [--out-dir <dir>]
//! ```
//!
//! For each strategy hash, writes a file
//! `<out-dir>/status/<dbcache-stem>/ram-<hash>` with one tab-separated line
//! per instance (solved and unsolved alike):
//!
//! ```text
//! finset_1__t13_finset_1.p\tTheorem\t0.007
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rusqlite::{Connection, OpenFlags};

// ---------------------------------------------------------------------------
// Success status sets (mirrors solverpy's TPTP_OK ∪ SMT_OK)
// ---------------------------------------------------------------------------

fn is_solved(status: &str) -> bool {
    matches!(status,
        "Theorem" | "Unsatisfiable" | "Satisfiable" |
        "CounterSatisfiable" | "ContradictoryAxioms" |
        "sat" | "unsat"
    )
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "ramparils-db", about = "Inspect ramparils .dbcache files")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Write per-strategy solved-instance lists to files.
    Solved {
        /// Path to the .dbcache file.
        dbcache: PathBuf,

        /// Output root directory (default: current directory).
        #[arg(long, default_value = ".")]
        out_dir: PathBuf,
    },

    /// Write per-strategy status/runtime tables to files (all instances).
    Status {
        /// Path to the .dbcache file.
        dbcache: PathBuf,

        /// Output root directory (default: current directory).
        #[arg(long, default_value = ".")]
        out_dir: PathBuf,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let args = Args::parse();
    match args.cmd {
        Cmd::Solved  { dbcache, out_dir } => cmd_solved(&dbcache, &out_dir),
        Cmd::Status  { dbcache, out_dir } => cmd_status(&dbcache, &out_dir),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn open_ro(dbcache: &Path) -> Result<Connection> {
    Connection::open_with_flags(dbcache, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open {}", dbcache.display()))
}

fn db_stem(dbcache: &Path) -> Result<&str> {
    dbcache
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("cannot derive stem from {}", dbcache.display()))
}

fn basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
}

fn write_dir(out_dir: &Path, subdir: &str, stem: &str) -> Result<PathBuf> {
    let dir = out_dir.join(subdir).join(stem);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// `solved` sub-command
// ---------------------------------------------------------------------------

fn cmd_solved(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let mut stmt = conn.prepare(
        "SELECT r.strategy_hash, i.path, s.status \
         FROM results r \
         JOIN instances i ON r.instance_id = i.id \
         JOIN statuses  s ON r.status_id  = s.id \
         ORDER BY r.strategy_hash, i.path",
    )?;

    let mut solved: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    for row in rows {
        let (hash, path, status) = row.context("failed to read row")?;
        if is_solved(&status) {
            solved.entry(hash).or_default().push(basename(&path).to_string());
        }
    }

    if solved.is_empty() {
        eprintln!("no solved instances found in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "solved", stem)?;
    for (hash, mut names) in solved {
        names.sort_unstable();
        names.dedup();
        let file_path = dir.join(format!("ram-{:016x}", hash as u64));
        let mut w = BufWriter::new(fs::File::create(&file_path)
            .with_context(|| format!("failed to create {}", file_path.display()))?);
        for name in &names { writeln!(w, "{name}")?; }
        println!("{} ({} solved)", file_path.display(), names.len());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `status` sub-command
// ---------------------------------------------------------------------------

fn cmd_status(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let mut stmt = conn.prepare(
        "SELECT r.strategy_hash, i.path, s.status, r.runtime \
         FROM results r \
         JOIN instances i ON r.instance_id = i.id \
         JOIN statuses  s ON r.status_id  = s.id \
         ORDER BY r.strategy_hash, i.path",
    )?;

    // BTreeMap<hash, Vec<(basename, status, runtime)>>
    let mut table: BTreeMap<i64, Vec<(String, String, f64)>> = BTreeMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
        ))
    })?;
    for row in rows {
        let (hash, path, status, runtime) = row.context("failed to read row")?;
        table.entry(hash).or_default()
            .push((basename(&path).to_string(), status, runtime));
    }

    if table.is_empty() {
        eprintln!("no results found in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "status", stem)?;
    for (hash, rows) in table {
        let file_path = dir.join(format!("ram-{:016x}", hash as u64));
        let mut w = BufWriter::new(fs::File::create(&file_path)
            .with_context(|| format!("failed to create {}", file_path.display()))?);
        for (name, status, runtime) in &rows {
            writeln!(w, "{name}\t{status}\t{runtime:.3}")?;
        }
        println!("{} ({} instances)", file_path.display(), rows.len());
    }
    Ok(())
}
