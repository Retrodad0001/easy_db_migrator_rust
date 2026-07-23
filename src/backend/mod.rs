/// The outcome of attempting to run a single migration script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMigrationResult {
    /// The script ran and its tracking row was inserted.
    MigrationScriptExecuted,
    /// The script was already recorded in the tracking table and was skipped.
    ScriptSkippedBecauseAlreadyRun,
    /// The run was cancelled before the script could execute.
    MigrationWasCancelled,
}

pub(crate) mod mssql;
pub(crate) mod postgres;
