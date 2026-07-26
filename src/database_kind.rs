use std::fmt;

/// Identifies which database dialect a migration run targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatabaseKind {
    /// Microsoft SQL Server, backed by `tiberius`.
    Mssql,
    /// PostgreSQL, backed by `sqlx`.
    Postgresql,
}

impl fmt::Display for DatabaseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            DatabaseKind::Mssql => "mssql",
            DatabaseKind::Postgresql => "postgresql",
        };
        f.write_str(name)
    }
}
