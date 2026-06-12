//! Persistent result cache backed by SQLite.
//!
//! Schema (normalized to keep `results` rows compact at scale):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS instances (
//!     id   INTEGER PRIMARY KEY,
//!     path TEXT UNIQUE NOT NULL
//! );
//! CREATE TABLE IF NOT EXISTS statuses (
//!     id     INTEGER PRIMARY KEY,
//!     status TEXT UNIQUE NOT NULL
//! );
//! CREATE TABLE IF NOT EXISTS results (
//!     strategy_hash INTEGER NOT NULL,
//!     instance_id   INTEGER NOT NULL,
//!     runtime       REAL    NOT NULL,
//!     quality       REAL    NOT NULL,
//!     status_id     INTEGER NOT NULL DEFAULT 0,
//!     cutoff        REAL    NOT NULL,
//!     PRIMARY KEY (strategy_hash, instance_id)
//! );
//! ```
//!
//! `quality` is the best-solution value reported by the solver (used when
//! `run_obj = quality`; for runtime-objective runs it is ignored but still
//! stored in case the cache is shared across scenario types).
//!
//! `status` holds the raw solver status string (e.g. `Theorem`, `Timeout`,
//! `sat`).  Status strings are interned into the `statuses` table so the hot
//! `results` rows only carry an integer FK.  `status_id = 0` means `UNKNOWN`
//! (used for rows inserted before status tracking was introduced).
//!
//! The `instances` table is populated once at startup via `load_instances()`,
//! which returns an in-memory `HashMap<String, i64>` used for all subsequent
//! queries — no path strings appear in hot-path SQL.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

use crate::params::Config;

const SYNTHETIC_TIMEOUT_QUALITY: f64 = 10_000_000.0;

/// A single cached solver result.
#[derive(Debug, Clone)]
pub struct CachedResult {
    pub runtime: f64,
    pub quality: f64,
    pub status: String,
}

pub struct Cache {
    conn: Connection,
    pub debug: bool,
    /// In-memory intern map: status string → statuses.id.
    status_cache: HashMap<String, i64>,
}

impl Cache {
    /// Open (or create) the SQLite DB at `path` and ensure the schema exists.
    /// Pass `":memory:"` for an in-process test database.
    pub fn open(path: &str, debug: bool) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open cache DB: {path}"))?;

        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            CREATE TABLE IF NOT EXISTS instances (
                id   INTEGER PRIMARY KEY,
                path TEXT UNIQUE NOT NULL
            );
            CREATE TABLE IF NOT EXISTS statuses (
                id     INTEGER PRIMARY KEY,
                status TEXT UNIQUE NOT NULL
            );
            INSERT OR IGNORE INTO statuses (id, status) VALUES (0, 'UNKNOWN');
            CREATE TABLE IF NOT EXISTS results (
                strategy_hash INTEGER NOT NULL,
                instance_id   INTEGER NOT NULL,
                runtime       REAL    NOT NULL,
                quality       REAL    NOT NULL,
                status_id     INTEGER NOT NULL DEFAULT 0,
                cutoff        REAL    NOT NULL,
                PRIMARY KEY (strategy_hash, instance_id)
            );
        ").context("failed to initialise cache schema")?;

        let columns = {
            let mut statement = conn.prepare("PRAGMA table_info(results)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        anyhow::ensure!(
            columns.iter().any(|column| column == "status_id")
                && columns.iter().any(|column| column == "cutoff"),
            "cache uses an incompatible schema; remove it and create a fresh cache"
        );

        crate::debug_line(debug, &format!("[{:8.2}s] cache: opened path={path}", crate::t()));
        Ok(Cache { conn, debug, status_cache: HashMap::new() })
    }

    /// Register all instance paths and return an in-memory `path → id` map.
    ///
    /// Called once at startup.  Uses `INSERT OR IGNORE` so existing rows are
    /// left untouched, then fetches ids for every path in one query.
    pub fn load_instances(&self, instances: &[String]) -> Result<HashMap<String, i64>> {
        {
            let mut stmt = self.conn.prepare_cached(
                "INSERT OR IGNORE INTO instances (path) VALUES (?1)"
            )?;
            for path in instances {
                stmt.execute(params![path])?;
            }
        }

        if instances.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = instances.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT path, id FROM instances WHERE path IN ({placeholders})"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let map = stmt
            .query_map(rusqlite::params_from_iter(instances.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .context("failed to load instance ids")?;
        crate::debug_line(self.debug, &format!("[{:8.2}s] cache: load_instances n={}", crate::t(), map.len()));
        Ok(map)
    }

    /// Bulk-fetch cached results for one strategy on a set of instances.
    ///
    /// Returns a map of `instance_id → CachedResult` for every cache hit;
    /// missing keys are cache misses.
    pub fn get_batch(
        &self,
        strategy_hash: u64,
        instance_ids: &[i64],
        requested_cutoff: f64,
    ) -> Result<HashMap<i64, CachedResult>> {
        if instance_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = instance_ids.iter().enumerate()
            .map(|(i, _)| format!("?{}", i + 2)) // ?1 = strategy_hash
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT r.instance_id, r.runtime, r.quality, s.status, r.cutoff \
             FROM results r JOIN statuses s ON r.status_id = s.id \
             WHERE r.strategy_hash = ?1 AND r.instance_id IN ({placeholders})"
        );
        let hash = strategy_hash as i64;
        let mut stmt = self.conn.prepare(&sql)?;
        let iter = stmt.query_map(
            rusqlite::params_from_iter(
                std::iter::once(&hash as &dyn rusqlite::types::ToSql)
                    .chain(instance_ids.iter().map(|id| id as &dyn rusqlite::types::ToSql))
            ),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    (
                        CachedResult {
                            runtime: row.get(1)?,
                            quality: row.get(2)?,
                            status: row.get(3)?,
                        },
                        row.get::<_, f64>(4)?,
                    ),
                ))
            },
        )?;
        let result = iter
            .filter_map(|row| match row {
                Ok((instance_id, (cached, stored_cutoff))) => {
                    adapt_cached_result(cached, stored_cutoff, requested_cutoff)
                        .map(|cached| Ok((instance_id, cached)))
                }
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .context("failed to query result cache")?;
        Ok(result)
    }

    /// Store a single result.
    pub fn put(
        &mut self,
        strategy_hash: u64,
        instance_id: i64,
        runtime: f64,
        quality: f64,
        status: &str,
        cutoff: f64,
    ) -> Result<()> {
        let status_id = self.intern_status(status)?;
        let existing = self
            .conn
            .query_row(
                "SELECT s.status, r.cutoff \
                 FROM results r JOIN statuses s ON r.status_id = s.id \
                 WHERE r.strategy_hash = ?1 AND r.instance_id = ?2",
                params![strategy_hash as i64, instance_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
            )
            .optional()?;
        let should_write = match existing {
            None => true,
            Some((existing_status, existing_cutoff)) => {
                existing_status.eq_ignore_ascii_case("timeout")
                    && (!status.eq_ignore_ascii_case("timeout") || cutoff > existing_cutoff)
            }
        };
        if !should_write {
            return Ok(());
        }

        self.conn.execute(
            "INSERT OR REPLACE INTO results \
             (strategy_hash, instance_id, runtime, quality, status_id, cutoff) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                strategy_hash as i64,
                instance_id,
                runtime,
                quality,
                status_id,
                cutoff
            ],
        ).context("failed to write to result cache")?;
        Ok(())
    }

    /// Intern a status string and return its integer id.
    ///
    /// Uses an in-memory map to avoid a SQL round-trip for repeated statuses
    /// (there are typically only a handful of distinct values per run).
    pub fn intern_status(&mut self, status: &str) -> Result<i64> {
        if let Some(&id) = self.status_cache.get(status) {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO statuses (status) VALUES (?1)",
            params![status],
        ).context("failed to intern status")?;
        let id: i64 = self.conn.query_row(
            "SELECT id FROM statuses WHERE status = ?1",
            params![status],
            |row| row.get(0),
        ).context("failed to fetch status id")?;
        self.status_cache.insert(status.to_string(), id);
        Ok(id)
    }
}

fn adapt_cached_result(
    cached: CachedResult,
    stored_cutoff: f64,
    requested_cutoff: f64,
) -> Option<CachedResult> {
    if cached.status.eq_ignore_ascii_case("timeout") {
        if stored_cutoff < requested_cutoff {
            return None;
        }
        return Some(CachedResult {
            runtime: requested_cutoff,
            ..cached
        });
    }

    if cached.runtime > requested_cutoff {
        return Some(CachedResult {
            runtime: requested_cutoff,
            quality: SYNTHETIC_TIMEOUT_QUALITY,
            status: "TIMEOUT".to_string(),
        });
    }

    Some(cached)
}

/// Deterministic u64 hash of a `Config`.
///
/// Keys are sorted before hashing so insertion order does not matter.
/// Uses `std::hash::DefaultHasher` — stable within a single binary build
/// but NOT portable across compiler versions or machines.
pub fn hash_config(config: &Config) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut pairs: Vec<(&String, &String)> = config.iter().collect();
    pairs.sort_unstable_by_key(|(k, _)| *k);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    pairs.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Cache {
        Cache::open(":memory:", false).unwrap()
    }

    #[test]
    fn open_creates_schema() {
        open();
    }

    #[test]
    fn load_instances_round_trip() {
        let cache = open();
        let paths = vec!["a.cnf".to_string(), "b.cnf".to_string()];
        let map = cache.load_instances(&paths).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("a.cnf"));
        // Idempotent
        let map2 = cache.load_instances(&paths).unwrap();
        assert_eq!(map["a.cnf"], map2["a.cnf"]);
    }

    #[test]
    fn get_batch_miss_then_hit() {
        let mut cache = open();
        let paths = vec!["p1.cnf".to_string(), "p2.cnf".to_string()];
        let ids = cache.load_instances(&paths).unwrap();
        let id1 = ids["p1.cnf"];
        let id2 = ids["p2.cnf"];

        assert!(cache.get_batch(42, &[id1, id2], 10.0).unwrap().is_empty());

        cache.put(42, id1, 1.23, 0.0, "Theorem", 10.0).unwrap();
        let hits = cache.get_batch(42, &[id1, id2], 10.0).unwrap();
        assert_eq!(hits.len(), 1);
        assert!((hits[&id1].runtime - 1.23).abs() < 1e-9);
        assert_eq!(hits[&id1].status, "Theorem");
        assert!(!hits.contains_key(&id2));
    }

    #[test]
    fn stronger_result_replaces_timeout() {
        let mut cache = open();
        let ids = cache.load_instances(&["x.cnf".to_string()]).unwrap();
        let id = ids["x.cnf"];
        cache.put(1, id, 2.0, 5.0, "Timeout", 2.0).unwrap();
        cache.put(1, id, 3.5, 7.0, "Timeout", 5.0).unwrap();
        cache.put(1, id, 1.5, 1.0, "Theorem", 5.0).unwrap();
        cache.put(1, id, 1.0, 0.0, "Timeout", 10.0).unwrap();

        let hits = cache.get_batch(1, &[id], 10.0).unwrap();
        assert!((hits[&id].runtime - 1.5).abs() < 1e-9);
        assert!((hits[&id].quality - 1.0).abs() < 1e-9);
        assert_eq!(hits[&id].status, "Theorem");
    }

    #[test]
    fn timeout_reuse_depends_on_cutoff() {
        let mut cache = open();
        let ids = cache.load_instances(&["x.cnf".to_string()]).unwrap();
        let id = ids["x.cnf"];
        cache.put(1, id, 5.0, 7.0, "timeout", 5.0).unwrap();

        let shorter = cache.get_batch(1, &[id], 2.0).unwrap();
        assert!((shorter[&id].runtime - 2.0).abs() < 1e-9);
        assert_eq!(shorter[&id].status, "timeout");
        assert!(cache.get_batch(1, &[id], 10.0).unwrap().is_empty());
    }

    #[test]
    fn success_beyond_cutoff_becomes_synthetic_timeout() {
        let mut cache = open();
        let ids = cache.load_instances(&["x.cnf".to_string()]).unwrap();
        let id = ids["x.cnf"];
        cache.put(1, id, 8.0, 0.0, "sat", 10.0).unwrap();

        let hits = cache.get_batch(1, &[id], 5.0).unwrap();
        assert!((hits[&id].runtime - 5.0).abs() < 1e-9);
        assert!((hits[&id].quality - SYNTHETIC_TIMEOUT_QUALITY).abs() < 1e-9);
        assert_eq!(hits[&id].status, "TIMEOUT");

        let full = cache.get_batch(1, &[id], 10.0).unwrap();
        assert!((full[&id].runtime - 8.0).abs() < 1e-9);
        assert_eq!(full[&id].status, "sat");
    }

    #[test]
    fn rejects_cache_without_cutoff_column() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(file.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE results (
                strategy_hash INTEGER NOT NULL,
                instance_id INTEGER NOT NULL,
                runtime REAL NOT NULL,
                quality REAL NOT NULL,
                status_id INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (strategy_hash, instance_id)
            );",
        )
        .unwrap();
        drop(conn);

        let error = Cache::open(file.path().to_str().unwrap(), false)
            .err()
            .expect("old cache schema should be rejected");
        assert!(error.to_string().contains("incompatible schema"));
    }

    #[test]
    fn intern_status_unknown_default() {
        let cache = open();
        // The UNKNOWN row (id=0) is pre-inserted by the schema setup.
        let hits = cache.get_batch(999, &[], 10.0).unwrap();
        assert!(hits.is_empty()); // nothing stored yet — just verifying open() succeeded
    }

    #[test]
    fn hash_config_stable_and_order_independent() {
        let c1: Config = [("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())].into();
        let c2: Config = [("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())].into();
        assert_eq!(hash_config(&c1), hash_config(&c2));

        let c3: Config = [("a".to_string(), "1".to_string()), ("b".to_string(), "9".to_string())].into();
        assert_ne!(hash_config(&c1), hash_config(&c3));
    }
}
