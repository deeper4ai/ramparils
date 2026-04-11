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
//! of instance paths solved by that strategy.
//!
//! "Solved" means the stored status is one of the known success tokens:
//! `Theorem`, `Unsatisfiable`, `Satisfiable`, `CounterSatisfiable`,
//! `ContradictoryAxioms` (TPTP) or `sat`, `unsat` (SMT-LIB2).

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
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let args = Args::parse();
    match args.cmd {
        Cmd::Solved { dbcache, out_dir } => cmd_solved(&dbcache, &out_dir),
    }
}

// ---------------------------------------------------------------------------
// `solved` sub-command
// ---------------------------------------------------------------------------

fn cmd_solved(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(dbcache, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open {}", dbcache.display()))?;

    // stem used as subdirectory name, e.g. "eprover-bushy010" from
    // "eprover-bushy010.dbcache"
    let stem = dbcache
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("cannot derive stem from {}", dbcache.display()))?;

    // One query: fetch all (hash, instance_path, status) rows.
    let mut stmt = conn.prepare(
        "SELECT r.strategy_hash, i.path, s.status \
         FROM results r \
         JOIN instances i ON r.instance_id = i.id \
         JOIN statuses  s ON r.status_id  = s.id \
         ORDER BY r.strategy_hash, i.path",
    )?;

    // Group solved instances by strategy hash.
    // BTreeMap keeps hashes in a deterministic order.
    let mut solved: BTreeMap<i64, Vec<String>> = BTreeMap::new();

    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;

    for row in rows {
        let (hash, path, status) = row.context("failed to read row")?;
        if is_solved(&status) {
            solved.entry(hash).or_default().push(path);
        }
    }

    if solved.is_empty() {
        eprintln!("no solved instances found in {}", dbcache.display());
        return Ok(());
    }

    // Write one file per strategy hash.
    let dir = out_dir.join("solved").join(stem);
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;

    for (hash, mut paths) in solved {
        paths.sort_unstable();
        paths.dedup();

        // Format hash as unsigned 16-char hex, matching the debug log output.
        let filename = format!("ram-{:016x}", hash as u64);
        let file_path = dir.join(&filename);
        let file = fs::File::create(&file_path)
            .with_context(|| format!("failed to create {}", file_path.display()))?;
        let mut w = BufWriter::new(file);
        for path in &paths {
            writeln!(w, "{path}")?;
        }
        println!("{} ({} solved)", file_path.display(), paths.len());
    }

    Ok(())
}
