use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};
use tokio_util::sync::CancellationToken;

use crate::{
    CRATE_VERSION, backend::RunMigrationResult, config::MigrationConfiguration, error::Error,
    script::Script,
};

const TRACKING_TABLE: &str = "DbMigrationsRun";

async fn connect_admin(config: &MigrationConfiguration) -> Result<PgConnection, Error> {
    let url = with_database(config.connection_string(), "postgres");
    Ok(PgConnection::connect(&url).await?)
}

async fn connect_target(config: &MigrationConfiguration) -> Result<PgConnection, Error> {
    let url = with_database(config.connection_string(), config.database_name());
    Ok(PgConnection::connect(&url).await?)
}

fn with_database(base_connection_string: &str, database_name: &str) -> String {
    format!(
        "{}/{database_name}",
        base_connection_string.trim_end_matches('/')
    )
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

pub(crate) async fn try_delete_database_if_exists(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut connection = connect_admin(config).await?;
    let query = format!(
        "DROP DATABASE IF EXISTS {}",
        quote_identifier(config.database_name())
    );
    connection.execute(AssertSqlSafe(query)).await?;
    Ok(())
}

pub(crate) async fn try_create_database_if_missing(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut connection = connect_admin(config).await?;

    let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
        .bind(config.database_name())
        .fetch_optional(&mut connection)
        .await?;

    if exists.is_none() {
        let query = format!(
            "CREATE DATABASE {}",
            quote_identifier(config.database_name())
        );
        connection.execute(AssertSqlSafe(query)).await?;
    }

    Ok(())
}

pub(crate) async fn try_ensure_tracking_table(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut connection = connect_target(config).await?;
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

pub(crate) async fn run_script(
    config: &MigrationConfiguration,
    script: &Script,
    executed_at: DateTime<Utc>,
    cancellation_token: &CancellationToken,
) -> Result<RunMigrationResult, Error> {
    if cancellation_token.is_cancelled() {
        return Ok(RunMigrationResult::MigrationWasCancelled);
    }

    let mut connection = connect_target(config).await?;

    let already_run: Option<i32> = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT id FROM {TRACKING_TABLE} WHERE filename = $1"
    )))
    .bind(script.filename())
    .fetch_optional(&mut connection)
    .await?;

    if already_run.is_some() {
        return Ok(RunMigrationResult::ScriptSkippedBecauseAlreadyRun);
    }

    let mut transaction = connection.begin().await?;

    transaction.execute(AssertSqlSafe(script.content())).await?;

    sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO {TRACKING_TABLE} (executed_at, filename, version) VALUES ($1, $2, $3)"
    )))
    .bind(executed_at)
    .bind(script.filename())
    .bind(CRATE_VERSION)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(RunMigrationResult::MigrationScriptExecuted)
}
