use std::fmt;

/// Identifies which database dialect a migration run targets.
///
/// A variant exists only when its backend feature is enabled, so a build that
/// took only `postgres` cannot name a SQL Server target at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatabaseKind {
    /// Microsoft SQL Server, backed by `tiberius`. Requires the `mssql` feature.
    #[cfg(feature = "mssql")]
    Mssql,
    /// PostgreSQL, backed by `sqlx`. Requires the `postgres` feature.
    #[cfg(feature = "postgres")]
    Postgresql,
}

impl fmt::Display for DatabaseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            #[cfg(feature = "mssql")]
            DatabaseKind::Mssql => "mssql",
            #[cfg(feature = "postgres")]
            DatabaseKind::Postgresql => "postgresql",
        };
        f.write_str(name)
    }
}
