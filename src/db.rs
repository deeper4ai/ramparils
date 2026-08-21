//! Export the contents of a `.dbcache` to files.
//!
//! The layout mirrors solverpy's database, so an export can be dropped into an
//! existing `solverpy_db/` and read by the same tooling:
//!
//! ```text
//! <out-dir>/solved/<dbcache-stem>/ram-<hash>    instance paths, one per line
//! <out-dir>/status/<dbcache-stem>/ram-<hash>    path <TAB> status <TAB> runtime <TAB> runhash
//! <out-dir>/confs/<dbcache-stem>/ram-<hash>     the configuration, as YAML
//! <out-dir>/runhashes/<dbcache-stem>.txt        ram-<hash> <SPACE> runhash <SPACE> n
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
//! `runhashes.txt` is computed fresh on every export, not stored back into
//! the cache (see ramparils-primo-cont's AGENTS.md discussion of why: an
//! incrementally-maintained column risks going stale if the instance set
//! grows later, and this keeps every export in `db.rs` read-only). Every hash
//! with at least one non-null runhash gets a line: instances with none
//! (timeout, error, unknown) are simply skipped when XOR-combining, not
//! treated as disqualifying the whole hash — requiring full coverage turned
//! out to exclude nearly everything, since a real benchmark almost always has
//! at least one instance nothing solves in time. `n` counts every result for
//! that hash, timeouts included — the same count `status()` would show — so
//! `n == instances` means fully evaluated against the whole benchmark, not
//! "runhash on every instance"; a hash can be fully evaluated while several
//! of its instances contributed nothing to the XOR.
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

/// One exported `status` row: `(path, status, runtime, runhash)`.
type StatusRow = (String, String, f64, Option<i64>);

/// One `path <TAB> status <TAB> runtime <TAB> runhash` line per result,
/// solved or not. `runhash` is empty when the row has none (a timeout,
/// error, or unknown result never carries one — see the `runhash` column
/// note in `cache.rs`), formatted as 16 lowercase hex digits otherwise, the
/// same convention as everywhere else it's printed (`log_incumbent`, the
/// wrapper's result line).
pub fn status(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let mut stmt = conn.prepare(
        "SELECT r.strategy_hash, i.path, s.status, r.runtime, r.runhash \
         FROM results r \
         JOIN instances i ON r.instance_id = i.id \
         JOIN statuses  s ON r.status_id  = s.id \
         ORDER BY r.strategy_hash, i.path",
    )?;

    let mut table: BTreeMap<i64, Vec<StatusRow>> = BTreeMap::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    for row in rows {
        let (hash, path, status, runtime, runhash) = row.context("failed to read row")?;
        table.entry(hash).or_default().push((path, status, runtime, runhash));
    }

    if table.is_empty() {
        println!("no results in {}", dbcache.display());
        return Ok(());
    }

    let dir = write_dir(out_dir, "status", stem)?;
    let (mut files, mut lines) = (0usize, 0usize);
    for (hash, rows) in table {
        let mut w = create(&dir.join(format!("ram-{:016x}", hash as u64)))?;
        for (path, status, runtime, runhash) in &rows {
            let runhash = runhash.map(|h| format!("{:016x}", h as u64)).unwrap_or_default();
            writeln!(w, "{path}\t{status}\t{runtime:.3}\t{runhash}")?;
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

/// One `ram-<hash> <SPACE> runhash <SPACE> n` line per hash that has at
/// least one non-null runhash — `ram-<hash>` matches the filenames under
/// `solved/`, `status/` and `confs/`. See the module doc for why this is
/// computed fresh here rather than stored back into `strategies`.
///
/// `runhash` is the XOR of every non-null value for that hash; instances with
/// none (timeout, error, unknown) are simply skipped when combining, **not**
/// treated as disqualifying the whole hash — requiring every instance to have
/// one would exclude nearly every hash in practice, since at least one
/// instance in a real benchmark almost always goes unsolved by everything.
///
/// `n`, unlike the XOR, counts **every** result for that hash, timeouts
/// included — the same count `status()`'s export would show for it (one line
/// per result there, regardless of status). So `n == instances` means fully
/// evaluated against the whole benchmark, not "runhash on every instance": a
/// hash can be fully evaluated (`n == instances`) while several of its
/// instances contributed nothing to the XOR because they timed out. Two lines
/// with the same `n` therefore share *coverage*, not necessarily the same
/// number of actual contributors — cross-check `status()`'s export when that
/// distinction matters.
pub fn runhashes(dbcache: &Path, out_dir: &Path) -> Result<()> {
    let conn = open_ro(dbcache)?;
    let stem = db_stem(dbcache)?;

    let mut stmt = conn.prepare("SELECT strategy_hash, runhash FROM results ORDER BY strategy_hash")?;
    let mut per_hash: BTreeMap<i64, (u64, usize, usize)> = BTreeMap::new();
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)))?;
    for row in rows {
        let (hash, runhash) = row.context("failed to read row")?;
        let entry = per_hash.entry(hash).or_insert((0, 0, 0));
        entry.2 += 1; // every result counts toward n, timeouts included
        if let Some(v) = runhash {
            entry.0 ^= v as u64;
            entry.1 += 1;
        }
    }
    per_hash.retain(|_, &mut (_, contributors, _)| contributors > 0);

    if per_hash.is_empty() {
        println!("no results with a runhash in {}", dbcache.display());
        return Ok(());
    }

    let dir = out_dir.join("runhashes");
    fs::create_dir_all(&dir).with_context(|| format!("failed to create directory {}", dir.display()))?;
    let path = dir.join(format!("{stem}.txt"));
    let mut w = create(&path)?;
    for (hash, (runhash, _contributors, n)) in &per_hash {
        writeln!(w, "ram-{:016x} {runhash:016x} {n}", *hash as u64)?;
    }
    println!("{} strategies exported to {}", per_hash.len(), path.display());
    Ok(())
}

/// Run all four exports over one cache.
///
/// What `ramparils db <cache>` does with no sub-command. Each writes its own
/// summary line, so the caller sees the same four lines as running them
/// separately. `confs` is written as YAML; use the sub-command for `--json`.
pub fn export_all(dbcache: &Path, out_dir: &Path) -> Result<()> {
    solved(dbcache, out_dir)?;
    status(dbcache, out_dir)?;
    confs(dbcache, out_dir, false)?;
    runhashes(dbcache, out_dir)
}

#[cfg(test)]
mod tests {
    use crate::cache::{Cache, hash_config};
    use crate::params::Config;

    #[test]
    fn status_export_includes_runhash_column() {
        let cache_file = tempfile::NamedTempFile::new().unwrap();
        let cache_path = cache_file.path().to_path_buf();
        let out_dir = tempfile::tempdir().unwrap();

        let config: Config = [("a".to_string(), "1".to_string())].into_iter().collect();
        let hash = hash_config(&config);
        {
            let mut cache = Cache::open(cache_path.to_str().unwrap(), false).unwrap();
            let ids = cache
                .load_instances(&["solved.p".to_string(), "timedout.p".to_string()])
                .unwrap();
            cache.put_strategy(hash, &config).unwrap();
            cache
                .put(hash, ids["solved.p"], 0.5, 0.0, "Theorem", 10.0, Some(0x3eff6fcf0e4d910d))
                .unwrap();
            cache.put(hash, ids["timedout.p"], 10.0, 0.0, "ResourceOut", 10.0, None).unwrap();
        }

        super::status(&cache_path, out_dir.path()).unwrap();

        let stem = cache_path.file_stem().unwrap().to_str().unwrap();
        let exported = out_dir.path().join("status").join(stem).join(format!("ram-{hash:016x}"));
        let lines: Vec<String> = std::fs::read_to_string(exported).unwrap().lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 2);

        let solved_line = lines.iter().find(|l| l.starts_with("solved.p")).unwrap();
        assert_eq!(solved_line, "solved.p\tTheorem\t0.500\t3eff6fcf0e4d910d");

        let timedout_line = lines.iter().find(|l| l.starts_with("timedout.p")).unwrap();
        assert_eq!(timedout_line, "timedout.p\tResourceOut\t10.000\t");
    }

    #[test]
    fn runhashes_export_n_counts_all_attempts_not_just_contributors() {
        let cache_file = tempfile::NamedTempFile::new().unwrap();
        let cache_path = cache_file.path().to_path_buf();
        let out_dir = tempfile::tempdir().unwrap();

        let complete: Config = [("a".to_string(), "1".to_string())].into_iter().collect();
        let complete_hash = hash_config(&complete);
        let partial_attempt: Config = [("a".to_string(), "2".to_string())].into_iter().collect();
        let partial_attempt_hash = hash_config(&partial_attempt);
        let partial_timeout: Config = [("a".to_string(), "3".to_string())].into_iter().collect();
        let partial_timeout_hash = hash_config(&partial_timeout);
        let all_timeout: Config = [("a".to_string(), "4".to_string())].into_iter().collect();
        let all_timeout_hash = hash_config(&all_timeout);

        {
            let mut cache = Cache::open(cache_path.to_str().unwrap(), false).unwrap();
            let ids = cache.load_instances(&["i1.p".to_string(), "i2.p".to_string()]).unwrap();

            // Both instances attempted and succeed: n=2, XORed.
            cache.put_strategy(complete_hash, &complete).unwrap();
            cache.put(complete_hash, ids["i1.p"], 0.1, 0.0, "Theorem", 10.0, Some(0xAAAA)).unwrap();
            cache.put(complete_hash, ids["i2.p"], 0.2, 0.0, "Theorem", 10.0, Some(0x5555)).unwrap();

            // Only one of two instances attempted at all: still included, n=1
            // -- not disqualified for missing coverage.
            cache.put_strategy(partial_attempt_hash, &partial_attempt).unwrap();
            cache.put(partial_attempt_hash, ids["i1.p"], 0.1, 0.0, "Theorem", 10.0, Some(0x1234)).unwrap();

            // Both attempted, one times out (null runhash): the timeout is
            // skipped when XOR-combining but still counts toward n (every
            // result, timeouts included) -- included, n=2, runhash from the
            // one instance that actually contributed.
            cache.put_strategy(partial_timeout_hash, &partial_timeout).unwrap();
            cache.put(partial_timeout_hash, ids["i1.p"], 0.1, 0.0, "Theorem", 10.0, Some(0x1234)).unwrap();
            cache.put(partial_timeout_hash, ids["i2.p"], 10.0, 0.0, "ResourceOut", 10.0, None).unwrap();

            // Both instances time out: nothing to XOR, must be excluded
            // entirely (n=0 carries no information, matching the wrapper's
            // own reasoning for an empty-selection hash).
            cache.put_strategy(all_timeout_hash, &all_timeout).unwrap();
            cache.put(all_timeout_hash, ids["i1.p"], 10.0, 0.0, "ResourceOut", 10.0, None).unwrap();
            cache.put(all_timeout_hash, ids["i2.p"], 10.0, 0.0, "ResourceOut", 10.0, None).unwrap();
        }

        super::runhashes(&cache_path, out_dir.path()).unwrap();

        let stem = cache_path.file_stem().unwrap().to_str().unwrap();
        let exported = out_dir.path().join("runhashes").join(format!("{stem}.txt"));
        let contents = std::fs::read_to_string(exported).unwrap();
        let mut lines: Vec<String> = contents.lines().map(str::to_string).collect();
        lines.sort();

        let mut expected = vec![
            format!("ram-{complete_hash:016x} {:016x} 2", 0xAAAAu64 ^ 0x5555u64),
            format!("ram-{partial_attempt_hash:016x} 0000000000001234 1"),
            format!("ram-{partial_timeout_hash:016x} 0000000000001234 2"),
        ];
        expected.sort();
        assert_eq!(lines, expected);
        assert!(
            !contents.contains(&format!("ram-{all_timeout_hash:016x}")),
            "a hash with zero non-null runhashes must not appear at all"
        );
    }
}
