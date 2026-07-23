use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    backend::{self, RunMigrationResult},
    clock::ClockMock,
    config::MigrationConfiguration,
    database_kind::DatabaseKind,
    script,
    script::Script,
};

enum RunOutcome {
    Completed { all_succeeded: bool },
    Cancelled,
}

/// Runs plain-SQL migration scripts against a database, tracking which ones have
/// already been applied so that re-running a migration is a true no-op.
pub struct DbMigrator {
    clock: Box<dyn ClockMock>,
    excluded_scripts: Vec<String>,
}

impl DbMigrator {
    /// Creates a migrator with an injectable [`ClockMock`] and the script filenames to
    /// exclude from the next migration run.
    /// The [`DatabaseKind`] is not stored on the migrator — it's passed explicitly to
    /// each method that needs to dispatch on it (e.g.
    /// [`try_delete_database_if_exists`](Self::try_delete_database_if_exists),
    /// [`try_apply_migrations`](Self::try_apply_migrations)).
    pub fn with_clock_mock(
        clock: impl ClockMock + 'static,
        excluded_scripts: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            clock: Box::new(clock),
            excluded_scripts: excluded_scripts.into_iter().collect(),
        }
    }

    /// Deletes the configured database if it exists. Use only in non-production
    /// environments, e.g. to reset state before an integration test run.
    pub async fn try_delete_database_if_exists(
        &self,
        kind: DatabaseKind,
        config: &MigrationConfiguration,
    ) -> bool {
        let result = match kind {
            DatabaseKind::Postgresql => {
                backend::postgres::try_delete_database_if_exists(config).await
            }
            DatabaseKind::Mssql => backend::mssql::try_delete_database_if_exists(config).await,
        };

        match result {
            Ok(()) => {
                info!("DeleteDatabaseIfExistAsync has executed");
                true
            }
            Err(error) => {
                error!(%error, "DeleteDatabaseIfExistAsync executed with error");
                false
            }
        }
    }

    /// Runs all pending migration scripts found in `config`'s scripts directory,
    /// creating the database and tracking table first if needed.
    ///
    /// A run cancelled before or during execution logs a warning and returns `true`
    /// — cancellation is not treated as failure.
    pub async fn try_apply_migrations(
        &self,
        kind: DatabaseKind,
        config: &MigrationConfiguration,
        cancellation_token: &CancellationToken,
    ) -> bool {
        if cancellation_token.is_cancelled() {
            warn!("migration process was canceled from the outside");
            return true;
        }

        info!(
            database = config.database_name(),
            "start running migrations for database"
        );
        info!(
            connection_string = config.connection_string(),
            "connection-string used"
        );

        let create_database_result = match kind {
            DatabaseKind::Postgresql => {
                backend::postgres::try_create_database_if_missing(config).await
            }
            DatabaseKind::Mssql => backend::mssql::try_create_database_if_missing(config).await,
        };
        if let Err(error) = create_database_result {
            error!(%error, "setup database executed with errors");
            error!("migration process executed with errors");
            return false;
        }
        info!("setup database executed successfully");

        let tracking_table_result = match kind {
            DatabaseKind::Postgresql => backend::postgres::try_ensure_tracking_table(config).await,
            DatabaseKind::Mssql => backend::mssql::try_ensure_tracking_table(config).await,
        };
        if let Err(error) = tracking_table_result {
            error!(%error, "setup versioning table executed with errors");
            error!("migration process executed with errors");
            return false;
        }
        info!("setup versioning table executed successfully");

        let scripts = match script::load_ordered_scripts(
            config.scripts_directory(),
            &self.excluded_scripts,
        ) {
            Ok(scripts) => scripts,
            Err(error) => {
                error!(%error, "One or more scripts could not be loaded, is the sequence patterns correct?");
                error!("migration process executed with errors");
                return false;
            }
        };

        match self
            .run_migration_scripts(kind, config, &scripts, cancellation_token)
            .await
        {
            RunOutcome::Cancelled => true,
            RunOutcome::Completed {
                all_succeeded: true,
            } => {
                info!("migration process executed successfully");
                true
            }
            RunOutcome::Completed {
                all_succeeded: false,
            } => {
                error!("migration process executed with errors");
                false
            }
        }
    }

    async fn run_migration_scripts(
        &self,
        kind: DatabaseKind,
        config: &MigrationConfiguration,
        scripts: &[Script],
        cancellation_token: &CancellationToken,
    ) -> RunOutcome {
        let mut skip_due_to_previous_error = false;

        for script in scripts {
            if skip_due_to_previous_error {
                warn!(
                    filename = script.filename(),
                    "script was skipped due to exception in previous script"
                );
                continue;
            }

            let executed_at = self.clock.now_utc();
            let result = match kind {
                DatabaseKind::Postgresql => {
                    backend::postgres::run_script(config, script, executed_at, cancellation_token)
                        .await
                }
                DatabaseKind::Mssql => {
                    backend::mssql::run_script(config, script, executed_at, cancellation_token)
                        .await
                }
            };

            match result {
                Ok(RunMigrationResult::MigrationWasCancelled) => {
                    warn!("migration process was canceled");
                    return RunOutcome::Cancelled;
                }
                Ok(RunMigrationResult::MigrationScriptExecuted) => {
                    info!(filename = script.filename(), "script was run");
                }
                Ok(RunMigrationResult::ScriptSkippedBecauseAlreadyRun) => {
                    info!(
                        filename = script.filename(),
                        "script was not run because script was already executed"
                    );
                }
                Err(error) => {
                    error!(%error, filename = script.filename(), "script was not completed due to exception");
                    skip_due_to_previous_error = true;
                }
            }
        }

        RunOutcome::Completed {
            all_succeeded: !skip_due_to_previous_error,
        }
    }
}
