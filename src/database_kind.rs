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
