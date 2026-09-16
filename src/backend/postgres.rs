use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};

use crate::{
    CRATE_VERSION, backend::run_migration_result_kind::RunMigrationResultKind,
    error_kind::ErrorKind, migration_configuration::MigrationConfiguration, script::Script,
};

const TRACKING_TABLE: &str = "DbMigrationsRun";

async fn try_connect_admin(
    migration_configuration: &MigrationConfiguration,
) -> Result<PgConnection, ErrorKind> {
    let pg_connect_options =
        PgConnectOptions::from_str(&migration_configuration.connection_string)?
            .database("postgres");
    Ok(PgConnection::connect_with(&pg_connect_options).await?)
}

async fn try_connect_target(
    migration_configuration: &MigrationConfiguration,
) -> Result<PgConnection, ErrorKind> {
    let pg_connect_options =
        PgConnectOptions::from_str(&migration_configuration.connection_string)?
            .database(&migration_configuration.database_name);
    Ok(PgConnection::connect_with(&pg_connect_options).await?)
}

#[inline]
fn determine_the_quoted_database_name(migration_configuration: &MigrationConfiguration) -> String {
    format!(
        "\"{}\"",
        migration_configuration.database_name.replace('"', "\"\"")
    )
}

#[inline]
fn determine_the_drop_database_query(migration_configuration: &MigrationConfiguration) -> String {
    format!(
        "DROP DATABASE IF EXISTS {}",
        determine_the_quoted_database_name(migration_configuration)
    )
}

#[inline]
fn determine_the_create_database_query(migration_configuration: &MigrationConfiguration) -> String {
    format!(
        "CREATE DATABASE {}",
        determine_the_quoted_database_name(migration_configuration)
    )
}

pub(crate) async fn try_delete_database_if_exists(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut connection = try_connect_admin(migration_configuration).await?;
    connection
        .execute(AssertSqlSafe(determine_the_drop_database_query(
            migration_configuration,
        )))
        .await?;
    Ok(())
}

pub(crate) async fn try_create_database_if_missing(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut connection = try_connect_admin(migration_configuration).await?;

    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(migration_configuration.database_name.as_str())
        .fetch_optional(&mut connection)
        .await?;

    if exists.is_none() {
        connection
            .execute(AssertSqlSafe(determine_the_create_database_query(
                migration_configuration,
            )))
            .await?;
    }

    Ok(())
}

pub(crate) async fn try_ensure_tracking_table(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut connection = try_connect_target(migration_configuration).await?;
    connection
        .execute(AssertSqlSafe(format!(
            "CREATE TABLE IF NOT EXISTS {TRACKING_TABLE} (
                id SERIAL PRIMARY KEY,
                executed_at TIMESTAMPTZ NOT NULL,
                filename VARCHAR NOT NULL,
                version VARCHAR NOT NULL
            )"
        )))
        .await?;
    Ok(())
}

pub(crate) async fn try_run_script(
    migration_configuration: &MigrationConfiguration,
    script: &Script,
    date_time: DateTime<Utc>,
    is_cancelled: &AtomicBool,
) -> Result<RunMigrationResultKind, ErrorKind> {
    if is_cancelled.load(Ordering::SeqCst) {
        return Ok(RunMigrationResultKind::MigrationWasCancelled);
    }

    let mut connection = try_connect_target(migration_configuration).await?;

    let already_run: Option<i32> = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT id FROM {TRACKING_TABLE} WHERE filename = $1"
    )))
    .bind(script.filename.as_str())
    .fetch_optional(&mut connection)
    .await?;

    if already_run.is_some() {
        return Ok(RunMigrationResultKind::ScriptSkippedBecauseAlreadyRun);
    }

    let mut transaction = connection.begin().await?;

    transaction
        .execute(AssertSqlSafe(script.content.as_str()))
        .await?;

    sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO {TRACKING_TABLE} (executed_at, filename, version) VALUES ($1, $2, $3)"
    )))
    .bind(date_time)
    .bind(script.filename.as_str())
    .bind(CRATE_VERSION)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(RunMigrationResultKind::MigrationScriptExecuted)
}
