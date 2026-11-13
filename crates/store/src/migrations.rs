//! Ordered, versioned schema migrations: the `NNNN_name.sql` files under
//! `migrations/` in this crate, embedded at build time, applied with the
//! `__migrations` bookkeeping.
//!
//! Never edit a shipped migration in place (a deployed database has recorded
//! its version and will not re-run it); add the next numbered file instead.

use std::collections::BTreeSet;

use recur_base::embed::Dir;
use rusqlite::Connection;

use crate::error::{Error, Result, SqlContext as _};

static MIGRATION_FILES: Dir = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// One parsed migration file.
#[derive(Debug, Clone)]
pub(crate) struct Migration {
    pub version: i64,
    pub name: String,
    pub stmts: String,
}

/// The embedded migrations in version order. A malformed filename is a
/// programming error (the files are ours), so it fails loudly rather than
/// silently skipping a schema change.
pub(crate) fn load_migrations() -> Result<Vec<Migration>> {
    let mut out = Vec::new();
    for f in MIGRATION_FILES.files() {
        let file = f.path;
        if !file.ends_with(".sql") {
            continue;
        }
        let (version, name) = parse_migration_name(file)?;
        let stmts = std::str::from_utf8(f.contents)
            .map_err(|_| Error::msg(format!("read migration {file}: not UTF-8")))?;
        out.push(Migration {
            version,
            name,
            stmts: stmts.to_owned(),
        });
    }
    out.sort_by_key(|m| m.version);
    for w in out.windows(2) {
        if w[0].version == w[1].version {
            return Err(Error::msg(format!(
                "duplicate migration version {} ({} / {})",
                w[1].version, w[0].name, w[1].name
            )));
        }
    }
    Ok(out)
}

/// Splits "0002_sessions.sql" into (2, "sessions").
pub(crate) fn parse_migration_name(file: &str) -> Result<(i64, String)> {
    let base = file.strip_suffix(".sql").unwrap_or(file);
    let Some((num, rest)) = base.split_once('_') else {
        return Err(Error::msg(format!(
            "migration {file:?}: want NNNN_name.sql"
        )));
    };
    let version = num
        .parse::<i64>()
        .map_err(|e| Error::msg(format!("migration {file:?}: bad version prefix: {e}")))?;
    Ok((version, rest.to_owned()))
}

/// Brings the database to the latest schema version: sets the connection
/// PRAGMAs, ensures `__migrations` exists, refuses a database that recorded
/// migrations this build does not ship, then applies each pending migration
/// in its own transaction.
pub(crate) fn run_migrations(conn: &mut Connection) -> Result<()> {
    // Connection-level PRAGMAs (every open, not persisted in a migration):
    // WAL for concurrent readers, foreign_keys for the ON DELETE CASCADE
    // relations the session tables declare.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .ctx(|| "set connection pragmas".into())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS __migrations (
		version    INTEGER PRIMARY KEY,
		name       TEXT NOT NULL,
		applied_at INTEGER NOT NULL
	)",
    )
    .ctx(|| "create __migrations".into())?;

    let migrations = load_migrations()?;
    let applied = applied_migrations(conn)?;
    let known: BTreeSet<i64> = migrations.iter().map(|m| m.version).collect();
    let unknown: Vec<i64> = applied.difference(&known).copied().collect();
    if !unknown.is_empty() {
        return Err(Error::NewerSchema { versions: unknown });
    }
    for m in &migrations {
        if !applied.contains(&m.version) {
            apply_migration(conn, m)?;
        }
    }
    Ok(())
}

fn applied_migrations(conn: &Connection) -> Result<BTreeSet<i64>> {
    let mut stmt = conn
        .prepare("SELECT version FROM __migrations")
        .ctx(|| "query __migrations".into())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .ctx(|| "query __migrations".into())?;
    let mut out = BTreeSet::new();
    for v in rows {
        out.insert(v.ctx(|| "scan __migrations".into())?);
    }
    Ok(out)
}

/// Runs one migration's statements and records its version in a single
/// transaction, so a partially applied migration is never recorded.
fn apply_migration(conn: &mut Connection, m: &Migration) -> Result<()> {
    let tx = conn
        .transaction()
        .ctx(|| format!("begin migration {} ({})", m.version, m.name))?;
    tx.execute_batch(&m.stmts)
        .ctx(|| format!("apply migration {} ({})", m.version, m.name))?;
    tx.execute(
        "INSERT INTO __migrations (version, name, applied_at) VALUES (?,?,?)",
        rusqlite::params![m.version, m.name, chrono::Utc::now().timestamp_millis()],
    )
    .ctx(|| format!("record migration {} ({})", m.version, m.name))?;
    tx.commit().bare()
}

/// The highest applied migration version (0 with none recorded).
#[cfg(test)]
pub(crate) fn schema_version(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT MAX(version) FROM __migrations", [], |r| {
        r.get::<_, Option<i64>>(0)
    })
    .map(Option::unwrap_or_default)
    .ctx(|| "read schema version".into())
}

/// The highest embedded migration version: what [`schema_version`] equals
/// after a successful open.
pub fn latest_schema_version() -> i64 {
    load_migrations()
        .ok()
        .and_then(|ms| ms.last().map(|m| m.version))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_migrations_are_ordered_and_named() {
        let ms = load_migrations().unwrap();
        assert_eq!(ms[0].version, 1);
        assert_eq!(ms[0].name, "v1_baseline");
        assert!(ms.windows(2).all(|w| w[0].version < w[1].version));
        assert!(
            !ms.iter().any(|m| m.version == 3),
            "0003 was skipped on purpose"
        );
        assert_eq!(latest_schema_version(), ms.last().unwrap().version);
    }

    #[test]
    fn parse_migration_names() {
        assert_eq!(
            parse_migration_name("0002_sessions.sql").unwrap(),
            (2, "sessions".to_owned())
        );
        assert_eq!(
            parse_migration_name("0010_project_groups.sql").unwrap(),
            (10, "project_groups".to_owned())
        );
        assert!(parse_migration_name("nounderscore.sql").is_err());
        assert!(parse_migration_name("x1_bad.sql").is_err());
    }
}
