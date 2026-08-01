use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{
    backend::{self, RunMigrationResult},
    clock::{ClockMock, SystemClock},
    config::MigrationConfiguration,
    database_kind::DatabaseKind,
    error::Error,
    script,
    script::Script,
};

const CANCELLED_FROM_OUTSIDE: &str = "migration process was canceled from the outside";
const EMPTY_CONNECTION_STRING: &str = "empty connectionstring is not valid";
const DELETE_DATABASE_FAILED: &str = "DeleteDatabaseIfExistAsync executed with error";
const SETUP_DATABASE_FAILED: &str = "setup database executed with errors";
const SETUP_TRACKING_TABLE_FAILED: &str = "setup versioning table executed with errors";
const SCRIPTS_NOT_LOADED: &str =
    "One or more scripts could not be loaded, is the sequence patterns correct?";
const MIGRATION_FAILED: &str = "migration process executed with errors";

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
    /// Creates a migrator backed by the system clock, taking the script filenames to
    /// exclude from the next migration run.
    pub fn new(excluded_scripts: impl IntoIterator<Item = String>) -> Self {
        Self::with_clock_mock(SystemClock, excluded_scripts)
    }

    /// Creates a migrator with an injectable [`ClockMock`] and the script filenames to
    /// exclude from the next migration run.
    ///
    /// This exists so a test can pin `executed_at` to a fixed instant. It is not part
    /// of the supported API — application code uses [`DbMigrator::new`].
    #[doc(hidden)]
    pub fn with_clock_mock(
        clock: impl ClockMock + 'static,
        excluded_scripts: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            clock: Box::new(clock),
            excluded_scripts: excluded_scripts.into_iter().collect(),
        }
    }

    /// Deletes the configured database if it exists.
    ///
    /// On failure the returned [`Error::MigrationFailed`] carries the same text that
    /// was reported through `tracing`.
    pub async fn try_delete_database_if_exists(
        &self,
        kind: DatabaseKind,
        config: &MigrationConfiguration,
    ) -> Result<(), Error> {
        let result = match kind {
            DatabaseKind::Postgresql => {
                backend::postgres::try_delete_database_if_exists(config).await
            }
            DatabaseKind::Mssql => backend::mssql::try_delete_database_if_exists(config).await,
        };

        match result {
            Ok(()) => {
                info!("DeleteDatabaseIfExistAsync has executed");
                Ok(())
            }
            Err(error) => {
                error!(%error, "{DELETE_DATABASE_FAILED}");
                Err(Error::MigrationFailed(DELETE_DATABASE_FAILED.to_string()))
            }
        }
    }

    /// Runs all pending migration scripts found in `config`'s scripts directory,
    /// creating the database and tracking table first if needed.
    ///
    /// A cancelled run is not a failure and returns `Ok(())`. On failure the returned
    /// [`Error::MigrationFailed`] carries the same text that was reported through
    /// `tracing`.
    pub async fn try_apply_migrations(
        &self,
        kind: DatabaseKind,
        config: &MigrationConfiguration,
        cancellation_token: &CancellationToken,
    ) -> Result<(), Error> {
        if cancellation_token.is_cancelled() {
            warn!("{CANCELLED_FROM_OUTSIDE}");
            return Ok(());
        }

        if config.connection_string().trim().is_empty() {
            error!("{EMPTY_CONNECTION_STRING}");
            return Err(Error::MigrationFailed(EMPTY_CONNECTION_STRING.to_string()));
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
            error!(%error, "{SETUP_DATABASE_FAILED}");
            error!("{MIGRATION_FAILED}");
            return Err(Error::MigrationFailed(SETUP_DATABASE_FAILED.to_string()));
        }
        info!("setup database executed successfully");

        let tracking_table_result = match kind {
            DatabaseKind::Postgresql => backend::postgres::try_ensure_tracking_table(config).await,
            DatabaseKind::Mssql => backend::mssql::try_ensure_tracking_table(config).await,
        };
        if let Err(error) = tracking_table_result {
            error!(%error, "{SETUP_TRACKING_TABLE_FAILED}");
            error!("{MIGRATION_FAILED}");
            return Err(Error::MigrationFailed(
                SETUP_TRACKING_TABLE_FAILED.to_string(),
            ));
        }
        info!("setup versioning table executed successfully");

        let scripts = match script::load_ordered_scripts(
            config.scripts_directory(),
            &self.excluded_scripts,
        ) {
            Ok(scripts) => scripts,
            Err(error) => {
                error!(%error, "{SCRIPTS_NOT_LOADED}");
                error!("{MIGRATION_FAILED}");
                return Err(Error::MigrationFailed(SCRIPTS_NOT_LOADED.to_string()));
            }
        };

        match self
            .run_migration_scripts(kind, config, &scripts, cancellation_token)
            .await
        {
            RunOutcome::Cancelled => Ok(()),
            RunOutcome::Completed {
                all_succeeded: true,
            } => {
                info!("migration process executed successfully");
                Ok(())
            }
            RunOutcome::Completed {
                all_succeeded: false,
            } => {
                error!("{MIGRATION_FAILED}");
                Err(Error::MigrationFailed(MIGRATION_FAILED.to_string()))
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

//TODO fill readme
//TODO add text github and some keyworld like in toml
//TODO Add demo console or check code api and add other test not happy path
//TODO Add action codeQL when public repo
