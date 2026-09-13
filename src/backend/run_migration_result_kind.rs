#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMigrationResultKind {
    MigrationScriptExecuted,
    ScriptSkippedBecauseAlreadyRun,
    MigrationWasCancelled,
}
