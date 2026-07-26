#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMigrationResult {
    MigrationScriptExecuted,
    ScriptSkippedBecauseAlreadyRun,
    MigrationWasCancelled,
}

pub(crate) mod mssql;
pub(crate) mod postgres;
