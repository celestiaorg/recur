//! Ports of the Go `internal/store` tests (`store_test.go`,
//! `sessions_test.go`, `schedules_test.go`, `paths*_test.go`; applog tests
//! live with applog). They reach into the store like the Go tests do.

use std::path::{Path, PathBuf};

use crate::Store;
use crate::db::Db;

mod compat_tests;
mod path_tests;
mod schedule_tests;
mod session_tests;
mod store_tests;

/// A store on `<dir>/recur.db`, demo-seeded on first open only (so a reopen
/// after clear_all stays empty, like a real user DB).
pub(crate) fn open_temp(dir: &Path) -> Store {
    let path = db_file(dir);
    let first = !path.exists();
    let s = Store::open(&path).unwrap_or_else(|e| panic!("open store: {e}"));
    if first {
        s.persist_demo_seed().unwrap();
    }
    s
}

pub(crate) fn db_file(dir: &Path) -> PathBuf {
    dir.join("recur.db")
}

pub(crate) fn db(s: &Store) -> &Db {
    s.db.as_ref().unwrap()
}

/// Runs raw SQL on the store's connection.
pub(crate) fn exec(s: &Store, q: &str) -> crate::Result<()> {
    db(s).with(|c| {
        c.execute_batch(q).map_err(|source| crate::Error::Sql {
            context: String::new(),
            source,
        })
    })
}

pub(crate) fn query_i64(s: &Store, q: &str) -> i64 {
    db(s)
        .with(|c| Ok(c.query_row(q, [], |r| r.get::<_, i64>(0)).unwrap()))
        .unwrap()
}

/// Runs statements on a bare migrated DB (Go's openDB) and closes it.
pub(crate) fn raw(path: &Path, stmts: &[&str]) {
    let d = Db::open(path).unwrap();
    for q in stmts {
        d.with(|c| {
            c.execute_batch(q)
                .unwrap_or_else(|e| panic!("simulate ({q}): {e}"));
            Ok(())
        })
        .unwrap();
    }
    d.close().unwrap();
}

pub(crate) fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}
