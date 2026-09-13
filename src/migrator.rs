use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tracing::{error, info, warn};

use crate::{
    backend::{self, run_migration_result_kind::RunMigrationResultKind},
    database_kind::DatabaseKind,
    error_kind::ErrorKind,
    migration_configuration::MigrationConfiguration,
    run_outcome_kind::RunOutcomeKind,
    script::Script,
};

/// Deletes the configured database if it exists.
///
/// On failure the returned [`ErrorKind::MigrationFailed`] carries the same text that
/// was reported through `tracing`.
pub async fn try_delete_database_if_exists(
    database_kind: DatabaseKind,
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let result = match database_kind {
        #[cfg(feature = "postgres")]
        DatabaseKind::Postgresql => {
            backend::postgres::try_delete_database_if_exists(migration_configuration).await
        }
        #[cfg(feature = "mssql")]
        DatabaseKind::Mssql => {
            backend::mssql::try_delete_database_if_exists(migration_configuration).await
        }
    };

    match result {
        Ok(()) => {
            info!("DeleteDatabaseIfExistAsync has executed");
            Ok(())
        }
        Err(error) => {
            const DELETE_DATABASE_FAILED: &str = "DeleteDatabaseIfExistAsync executed with error";
            error!(%error, "{DELETE_DATABASE_FAILED}");
            Err(ErrorKind::MigrationFailed(
                DELETE_DATABASE_FAILED.to_string(),
            ))
        }
    }
}

/// Runs all pending migration scripts found in the configuration's scripts
/// directory, creating the database and tracking table first if needed.
///
/// The run reads `is_cancelled` before it starts and between scripts, and stops
/// when it reads `true`. A cancelled run is not a failure and returns `Ok(())`. On
/// failure the returned [`ErrorKind::MigrationFailed`] carries the same text that
/// was reported through `tracing`.
pub async fn try_apply_migrations(
    database_kind: DatabaseKind,
    migration_configuration: &MigrationConfiguration,
    is_cancelled: &AtomicBool,
) -> Result<(), ErrorKind> {
    if is_cancelled.load(Ordering::SeqCst) {
        warn!("migration process was canceled from the outside");
        return Ok(());
    }

    info!(
        database = migration_configuration.database_name.as_str(),
        "start running migrations for database"
    );
    info!(
        connection_string = migration_configuration.connection_string.as_str(),
        "connection-string used"
    );

    const MIGRATION_FAILED: &str = "migration process executed with errors";

    let create_database_result = match database_kind {
        #[cfg(feature = "postgres")]
        DatabaseKind::Postgresql => {
            backend::postgres::try_create_database_if_missing(migration_configuration).await
        }
        #[cfg(feature = "mssql")]
        DatabaseKind::Mssql => {
            backend::mssql::try_create_database_if_missing(migration_configuration).await
        }
    };
    if let Err(error) = create_database_result {
        const SETUP_DATABASE_FAILED: &str = "setup database executed with errors";
        error!(%error, "{SETUP_DATABASE_FAILED}");
        error!("{MIGRATION_FAILED}");
        return Err(ErrorKind::MigrationFailed(
            SETUP_DATABASE_FAILED.to_string(),
        ));
    }
    info!("setup database executed successfully");

    let tracking_table_result = match database_kind {
        #[cfg(feature = "postgres")]
        DatabaseKind::Postgresql => {
            backend::postgres::try_ensure_tracking_table(migration_configuration).await
        }
        #[cfg(feature = "mssql")]
        DatabaseKind::Mssql => {
            backend::mssql::try_ensure_tracking_table(migration_configuration).await
        }
    };
    if let Err(error) = tracking_table_result {
        const SETUP_TRACKING_TABLE_FAILED: &str = "setup versioning table executed with errors";
        error!(%error, "{SETUP_TRACKING_TABLE_FAILED}");
        error!("{MIGRATION_FAILED}");
        return Err(ErrorKind::MigrationFailed(
            SETUP_TRACKING_TABLE_FAILED.to_string(),
        ));
    }
    info!("setup versioning table executed successfully");

    let scripts_result = try_list_the_script_filenames(&migration_configuration.scripts_directory)
        .map(|filenames| {
            determine_the_included_filenames(filenames, &migration_configuration.excluded_scripts)
        })
        .and_then(|filenames| {
            try_read_the_scripts(&migration_configuration.scripts_directory, &filenames)
        })
        .and_then(try_determine_the_ordered_scripts);
    let scripts = match scripts_result {
        Ok(scripts) => scripts,
        Err(error) => {
            const SCRIPTS_NOT_LOADED: &str =
                "One or more scripts could not be loaded, is the sequence patterns correct?";
            error!(%error, "{SCRIPTS_NOT_LOADED}");
            error!("{MIGRATION_FAILED}");
            return Err(ErrorKind::MigrationFailed(SCRIPTS_NOT_LOADED.to_string()));
        }
    };

    match run_migration_scripts(
        database_kind,
        migration_configuration,
        &scripts,
        is_cancelled,
    )
    .await
    {
        RunOutcomeKind::Cancelled => Ok(()),
        RunOutcomeKind::Completed {
            all_succeeded: true,
        } => {
            info!("migration process executed successfully");
            Ok(())
        }
        RunOutcomeKind::Completed {
            all_succeeded: false,
        } => {
            error!("{MIGRATION_FAILED}");
            Err(ErrorKind::MigrationFailed(MIGRATION_FAILED.to_string()))
        }
    }
}

async fn run_migration_scripts(
    database_kind: DatabaseKind,
    migration_configuration: &MigrationConfiguration,
    scripts: &[Script],
    is_cancelled: &AtomicBool,
) -> RunOutcomeKind {
    let mut skip_due_to_previous_error = false;

    for script in scripts {
        if skip_due_to_previous_error {
            warn!(
                filename = script.filename.as_str(),
                "script was skipped due to exception in previous script"
            );
            continue;
        }

        let date_time = Utc::now();
        let result = match database_kind {
            #[cfg(feature = "postgres")]
            DatabaseKind::Postgresql => {
                backend::postgres::try_run_script(
                    migration_configuration,
                    script,
                    date_time,
                    is_cancelled,
                )
                .await
            }
            #[cfg(feature = "mssql")]
            DatabaseKind::Mssql => {
                backend::mssql::try_run_script(
                    migration_configuration,
                    script,
                    date_time,
                    is_cancelled,
                )
                .await
            }
        };

        match result {
            Ok(RunMigrationResultKind::MigrationWasCancelled) => {
                warn!("migration process was canceled");
                return RunOutcomeKind::Cancelled;
            }
            Ok(RunMigrationResultKind::MigrationScriptExecuted) => {
                info!(filename = script.filename.as_str(), "script was run");
            }
            Ok(RunMigrationResultKind::ScriptSkippedBecauseAlreadyRun) => {
                info!(
                    filename = script.filename.as_str(),
                    "script was not run because script was already executed"
                );
            }
            Err(error) => {
                error!(%error, filename = script.filename.as_str(), "script was not completed due to exception");
                skip_due_to_previous_error = true;
            }
        }
    }

    RunOutcomeKind::Completed {
        all_succeeded: !skip_due_to_previous_error,
    }
}

fn try_list_the_script_filenames(path: &Path) -> Result<Vec<String>, ErrorKind> {
    let entries = std::fs::read_dir(path).map_err(|source| ErrorKind::ScriptsDirectory {
        path: path.to_path_buf(),
        source,
    })?;

    let mut filenames = Vec::new();
    for entry in entries {
        let entry_path = entry
            .map_err(|source| ErrorKind::ScriptsDirectory {
                path: path.to_path_buf(),
                source,
            })?
            .path();
        if !entry_path.is_file() {
            continue;
        }
        filenames.push(
            entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string(),
        );
    }
    Ok(filenames)
}

#[inline]
fn determine_the_included_filenames(
    filenames: Vec<String>,
    excluded_scripts: &[String],
) -> Vec<String> {
    filenames
        .into_iter()
        .filter(|filename| !excluded_scripts.contains(filename))
        .collect()
}

fn try_read_the_scripts(
    path: &Path,
    filenames: &[String],
) -> Result<Vec<(String, String)>, ErrorKind> {
    let mut script_files = Vec::new();
    for filename in filenames {
        let script_path = path.join(filename);
        let content = std::fs::read_to_string(&script_path).map_err(|source| {
            ErrorKind::ScriptsDirectory {
                path: script_path.clone(),
                source,
            }
        })?;
        script_files.push((filename.clone(), content));
    }
    Ok(script_files)
}

#[inline]
fn try_determine_the_ordered_scripts(
    script_files: Vec<(String, String)>,
) -> Result<Vec<Script>, ErrorKind> {
    let mut scripts = script_files
        .into_iter()
        .map(|(filename, content)| Script::new(filename, content))
        .collect::<Result<Vec<Script>, ErrorKind>>()?;
    scripts.sort_by(|a, b| {
        a.date_part
            .cmp(&b.date_part)
            .then(a.sequence_part.cmp(&b.sequence_part))
    });
    Ok(scripts)
}
