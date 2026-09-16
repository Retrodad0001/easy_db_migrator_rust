use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Once;

use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, Row};
use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;

use super::{TrackingRow, expect_ok, sweep_leftover_containers};

pub(crate) async fn start_the_container() -> (ContainerAsync<GenericImage>, String) {
    const TEST_LABEL: &str = "easy_db_migrator_rust_postgres_test";
    static SWEPT: Once = Once::new();
    SWEPT.call_once(|| sweep_leftover_containers(TEST_LABEL));

    let container = expect_ok(
        GenericImage::new("postgres", "17-alpine")
            .with_wait_for(WaitFor::message_on_stderr(
                "database system is ready to accept connections",
            ))
            .with_env_var("POSTGRES_DB", "postgres")
            .with_env_var("POSTGRES_USER", "postgres")
            .with_env_var("POSTGRES_PASSWORD", "postgres")
            .with_label(TEST_LABEL, "1")
            .start()
            .await,
        "failed to start postgres container",
    );
    let host = expect_ok(container.get_host().await, "failed to get host");
    let port = expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );

    (
        container,
        format!("postgres://postgres:postgres@{host}:{port}"),
    )
}

#[inline]
pub(crate) fn determine_the_fixtures_path(fixture_directory: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/postgres")
        .join(fixture_directory)
}

async fn connect(connection_string: &str, database_name: &str) -> PgConnection {
    let pg_connect_options = expect_ok(
        PgConnectOptions::from_str(connection_string),
        "invalid postgres connection string",
    )
    .database(database_name);
    expect_ok(
        PgConnection::connect_with(&pg_connect_options).await,
        "failed to connect to postgres",
    )
}

pub(crate) async fn is_database_existing(connection_string: &str, database_name: &str) -> bool {
    let mut connection = connect(connection_string, "postgres").await;
    let exists: Option<i32> = expect_ok(
        sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
            .bind(database_name)
            .fetch_optional(&mut connection)
            .await,
        "failed to check database existence",
    );
    exists.is_some()
}

pub(crate) async fn is_query_running(connection_string: &str, text: &str) -> bool {
    let mut connection = connect(connection_string, "postgres").await;
    let count: i64 = expect_ok(
        sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE query LIKE $1 AND pid <> pg_backend_pid()",
        )
        .bind(format!("%{text}%"))
        .fetch_one(&mut connection)
        .await,
        "failed to read pg_stat_activity",
    );
    count > 0
}

pub(crate) async fn is_database_accepting_connections(
    connection_string: &str,
    database_name: &str,
) -> bool {
    let mut connection = connect(connection_string, "postgres").await;
    let is_accepting: Option<bool> = expect_ok(
        sqlx::query_scalar("SELECT datallowconn FROM pg_database WHERE datname = $1")
            .bind(database_name)
            .fetch_optional(&mut connection)
            .await,
        "failed to read whether the database accepts connections",
    );
    is_accepting.unwrap_or(false)
}

pub(crate) async fn create_the_database(connection_string: &str, database_name: &str) {
    let mut connection = connect(connection_string, "postgres").await;
    expect_ok(
        connection
            .execute(AssertSqlSafe(format!(
                "CREATE DATABASE \"{database_name}\""
            )))
            .await,
        "failed to create the database up front",
    );
}

pub(crate) async fn execute_in_the_database(
    connection_string: &str,
    database_name: &str,
    sql: &str,
) {
    let mut connection = connect(connection_string, database_name).await;
    expect_ok(
        connection.execute(AssertSqlSafe(sql.to_string())).await,
        "failed to run setup statement",
    );
}

pub(crate) async fn set_whether_the_database_accepts_connections(
    connection_string: &str,
    database_name: &str,
    is_accepting: bool,
) {
    let mut connection = connect(connection_string, "postgres").await;
    expect_ok(
        connection
            .execute(AssertSqlSafe(format!(
                "ALTER DATABASE \"{database_name}\" WITH ALLOW_CONNECTIONS {is_accepting}"
            )))
            .await,
        "failed to change whether the database accepts connections",
    );
}

pub(crate) async fn is_table_existing(
    connection_string: &str,
    database_name: &str,
    table_name: &str,
) -> bool {
    let mut connection = connect(connection_string, database_name).await;
    let regclass: Option<String> = expect_ok(
        sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(table_name)
            .fetch_one(&mut connection)
            .await,
        "failed to check table existence",
    );
    regclass.is_some()
}

pub(crate) async fn read_the_user_table_names(
    connection_string: &str,
    database_name: &str,
) -> Vec<String> {
    let mut connection = connect(connection_string, database_name).await;
    let rows = expect_ok(
        sqlx::query(
            "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&mut connection)
        .await,
        "failed to list user tables",
    );
    rows.into_iter().map(|row| row.get("tablename")).collect()
}

pub(crate) async fn read_the_tracking_rows(
    connection_string: &str,
    database_name: &str,
) -> Vec<TrackingRow> {
    let mut connection = connect(connection_string, database_name).await;
    let rows = expect_ok(
        sqlx::query("SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id")
            .fetch_all(&mut connection)
            .await,
        "failed to query tracking table",
    );
    rows.into_iter()
        .map(|row| TrackingRow {
            filename: row.get("filename"),
            executed_at: row.get("executed_at"),
            version: row.get("version"),
        })
        .collect()
}
