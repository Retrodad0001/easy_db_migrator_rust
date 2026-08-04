#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMigrationResult {
    MigrationScriptExecuted,
    ScriptSkippedBecauseAlreadyRun,
    MigrationWasCancelled,
}

#[cfg(feature = "mssql")]
pub(crate) mod mssql;
#[cfg(feature = "postgres")]
pub(crate) mod postgres;
