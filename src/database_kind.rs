use std::fmt;

/// Identifies which database dialect a migration run targets.
///
/// Both backends are always compiled in; pass this in explicitly wherever code
/// needs to know or branch on the database kind — see
/// [`DbMigrator::try_apply_migrations`](crate::DbMigrator::try_apply_migrations).
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
