//! Export the contents of a `.dbcache` to files.
//!
//! The layout mirrors solverpy's database, so an export can be dropped into an
//! existing `solverpy_db/` and read by the same tooling:
//!
//! ```text
//! <out-dir>/solved/<dbcache-stem>/ram-<hash>    instance paths, one per line
//! <out-dir>/status/<dbcache-stem>/ram-<hash>    path <TAB> status <TAB> runtime
//! <out-dir>/confs/<dbcache-stem>/ram-<hash>     the configuration, as YAML
//! ```
//!
//! `confs/` is deliberately *not* solverpy's `strats/`. A strategy file there
//! holds a solver command line; a conf file here holds a parameter assignment,
//! which only means anything against the parameter space it was tuned in.
//!
//! A conf file records the **active** configuration — the parameters that
//! actually reached the target algorithm. Inactive conditional parameters are
//! absent, because the cache keys on the active configuration, which is
//! exactly what collapses a guarded sub-space to a single entry. So a conf
//! file is a faithful record of what ran, and it is *not* a complete
//! configuration: `initial_config_file` requires every parameter in the space,
//! inactive ones included, and will reject a conf file that omits any.
//!
//! Everything here opens the cache read-only.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

use crate::params::{Config, config_to_yaml};

/// Solver statuses counted as a success by [`solved`].
///
/// The union of TPTP's and SMT-LIB's success tokens, matching solverpy's
/// `TPTP_OK | SMT_OK`. A target algorithm reporting anything else is not
/// recognised here: RamParILS itself stores the status verbatim and never
/// interprets it when scoring, so this list is `solved`'s own convention and
/// nothing else in the tuner depends on it.
pub const SOLVED_STATUSES: [&str; 7] = [
    "Theorem",
    "Unsatisfiable",
    "Satisfiable",
    "CounterSatisfiable",
    "ContradictoryAxioms",
    "sat",
    "unsat",
];

fn is_solved(status: &str) -> bool {
    SOLVED_STATUSES.contains(&status)
}

fn open_ro(dbcache: &Path) -> Result<Connection> {
    Connection::open_with_flags(dbcache, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open cache DB: {}", dbcache.display()))
}

fn db_stem(dbcache: &Path) -> Result<&str> {
    dbcache
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("cannot derive a name from {}", dbcache.display()))
}

fn write_dir(out_dir: &Path, subdir: &str, stem: &str) -> Result<PathBuf> {
    let dir = out_dir.join(subdir).join(stem);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create directory {}", dir.display()))?;
    Ok(dir)
}

fn create(path: &Path) -> Result<BufWriter<fs::File>> {
    Ok(BufWriter::new(
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?,
    ))
}

/// One line per solved instance, as the full instance path the cache recorded.
pub fn solved(dbcache: &Path, out_dir: &Path) -> Result<()> {
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
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (hash, path, status) = row.context("failed to read row")?;
        if is_solved(&status) {
            solved.entry(hash).or_default().push(path);
        }
    }

    if solved.is_empty() {
        println!("no solved instances in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "solved", stem)?;
    let (mut files, mut lines) = (0usize, 0usize);
    for (hash, mut paths) in solved {
        paths.sort_unstable();
        paths.dedup();
        let mut w = create(&dir.join(format!("ram-{:016x}", hash as u64)))?;
        for path in &paths {
            writeln!(w, "{path}")?;
        }
        files += 1;
        lines += paths.len();
    }
    println!("{files} exported to {}/ ({lines} solved instances)", dir.display());
    Ok(())
}

/// One `path <TAB> status <TAB> runtime` line per result, solved or not.
pub fn status(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let mut stmt = conn.prepare(
        "SELECT r.strategy_hash, i.path, s.status, r.runtime \
         FROM results r \
         JOIN instances i ON r.instance_id = i.id \
         JOIN statuses  s ON r.status_id  = s.id \
         ORDER BY r.strategy_hash, i.path",
    )?;

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
        table.entry(hash).or_default().push((path, status, runtime));
    }

    if table.is_empty() {
        println!("no results in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "status", stem)?;
    let (mut files, mut lines) = (0usize, 0usize);
    for (hash, rows) in table {
        let mut w = create(&dir.join(format!("ram-{:016x}", hash as u64)))?;
        for (path, status, runtime) in &rows {
            writeln!(w, "{path}\t{status}\t{runtime:.3}")?;
        }
        files += 1;
        lines += rows.len();
    }
    println!("{files} exported to {}/ ({lines} results)", dir.display());
    Ok(())
}

/// One file per strategy hash, holding the configuration behind it.
///
/// Caches written before the `strategies` table existed have no rows here;
/// those hashes can only be recovered by enumerating the parameter space,
/// which needs the space to be small and the hash to be reproducible by the
/// current toolchain.
pub fn confs(dbcache: &Path, out_dir: &Path, as_json: bool) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let table_exists: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='strategies'")?
        .exists([])?;
    if !table_exists {
        println!(
            "{} predates the strategies table: no configurations recorded",
            dbcache.display()
        );
        return Ok(());
    }

    let mut stmt = conn.prepare("SELECT hash, config FROM strategies ORDER BY hash")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;

    let mut entries: Vec<(i64, String)> = Vec::new();
    for row in rows {
        entries.push(row.context("failed to read row")?);
    }

    if entries.is_empty() {
        println!("no configurations recorded in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "confs", stem)?;
    let mut files = 0usize;
    for (hash, json) in entries {
        let body = if as_json {
            format!("{json}\n")
        } else {
            let config: Config = serde_json::from_str(&json)
                .with_context(|| format!("corrupt strategy record for hash {:016x}", hash as u64))?;
            config_to_yaml(&config)?
        };
        let mut w = create(&dir.join(format!("ram-{:016x}", hash as u64)))?;
        write!(w, "{body}")?;
        files += 1;
    }
    println!("{files} exported to {}/", dir.display());
    Ok(())
}

/// Run all three exports over one cache.
///
/// What `ramparils db <cache>` does with no sub-command. Each writes its own
/// summary line, so the caller sees the same three lines as running them
/// separately. `confs` is written as YAML; use the sub-command for `--json`.
pub fn export_all(dbcache: &Path, out_dir: &Path) -> Result<()> {
    solved(dbcache, out_dir)?;
    status(dbcache, out_dir)?;
    confs(dbcache, out_dir, false)
}
