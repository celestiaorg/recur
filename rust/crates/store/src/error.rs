use std::fmt;

/// A store failure. Messages follow Go's `fmt.Errorf` wrapping
/// (`upsert project "x": <sqlite error>`).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A SQLite error, with Go's context prefix (empty when Go returned the
    /// driver error bare).
    Sql {
        context: String,
        #[source]
        source: rusqlite::Error,
    },
    /// Every operation after [`crate::Store::close`] (Go's
    /// `sql: database is closed`).
    Closed,
    /// The database records migrations this build does not ship: it was
    /// written by a newer Recur, so it is never opened for writing.
    NewerSchema { versions: Vec<i64> },
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },
    /// Writing `.recur/project.toml` failed.
    ProjectFile(#[from] recur_project::projfile::WriteError),
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// Validation and not-found errors, with Go's message text.
    Msg(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let wrapped = |f: &mut fmt::Formatter<'_>, ctx: &str, e: &dyn fmt::Display| {
            if ctx.is_empty() {
                write!(f, "{e}")
            } else {
                write!(f, "{ctx}: {e}")
            }
        };
        match self {
            Error::Sql { context, source } => wrapped(f, context, source),
            Error::Json { context, source } => wrapped(f, context, source),
            Error::Io { context, source } => wrapped(f, context, source),
            Error::Closed => f.write_str("sql: database is closed"),
            Error::NewerSchema { versions } => {
                let list: Vec<String> = versions.iter().map(i64::to_string).collect();
                write!(
                    f,
                    "recur.db has schema migration(s) {} that this build does not know; it was written by a newer Recur",
                    list.join(", ")
                )
            }
            Error::ProjectFile(e) => write!(f, "{e}"),
            Error::Msg(m) => f.write_str(m),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn msg(m: impl Into<String>) -> Self {
        Error::Msg(m.into())
    }

    /// Whether this is a SQLite UNIQUE constraint failure.
    pub fn is_unique_constraint(&self) -> bool {
        matches!(
            self,
            Error::Sql { source: rusqlite::Error::SqliteFailure(e, _), .. }
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        )
    }
}

/// Wraps a rusqlite result with Go's error context.
pub(crate) trait SqlContext<T> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T>;
    fn bare(self) -> Result<T>;
}

impl<T> SqlContext<T> for rusqlite::Result<T> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Sql {
            context: context(),
            source,
        })
    }

    fn bare(self) -> Result<T> {
        self.map_err(|source| Error::Sql {
            context: String::new(),
            source,
        })
    }
}

pub(crate) fn json_err(context: impl Into<String>) -> impl FnOnce(serde_json::Error) -> Error {
    let context = context.into();
    move |source| Error::Json { context, source }
}
