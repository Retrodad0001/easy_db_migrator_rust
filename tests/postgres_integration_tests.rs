#![allow(clippy::panic, missing_docs)]

use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use easy_db_migrator_rust::{
    CancellationToken, ClockMock, DatabaseKind, DbMigrator, Error, MigrationConfiguration,
};
use rand::RngExt;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection, Row};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tracing_test::traced_test;

macro_rules! assert_logged {
    ($level:expr, $message:expr) => {
        logs_assert(|lines: &[&str]| {
            let expected_level: &str = $level;
            let expected_message: &str = $message;
            let matching: Vec<&&str> = lines
                .iter()
                .filter(|line| line.contains(expected_message))
                .collect();
            if matching.is_empty() {
                return Err(format!(
                    "expected a log record containing {expected_message:?}, found none"
                ));
            }
            if matching
                .iter()
                .any(|line| line.contains(&format!(" {expected_level} ")))
            {
                Ok(())
            } else {
                Err(format!(
                    "expected {expected_message:?} to be logged at {expected_level}, found {matching:?}"
                ))
            }
        });
    };
}

struct FixedClockMock(DateTime<Utc>);

impl ClockMock for FixedClockMock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Debug)]
struct TrackingRow {
    filename: String,
    executed_at: DateTime<Utc>,
    version: String,
}

fn test_expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{context}: {error:?}"),
    }
}

fn test_expect_migration_error(result: Result<(), Error>, context: &str) -> String {
    match result {
        Ok(()) => panic!("{context}"),
        Err(error) => error.to_string(),
    }
}

fn test_expect_some<T>(value: Option<T>, context: &str) -> T {
    match value {
        Some(value) => value,
        None => panic!("{context}"),
    }
}

fn test_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/postgres/test_scripts")
}

fn test_failure_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/postgres/test_scripts_failure")
}

fn test_random_database_name() -> String {
    let suffix: u32 = rand::rng().random_range(100_000..999_999);
    format!("test{suffix}")
}

fn test_datetime(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> DateTime<Utc> {
    test_expect_some(
        Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
            .single(),
        "not a valid unambiguous UTC datetime",
    )
}

async fn test_database_exists(config: &MigrationConfiguration) -> bool {
    let admin_url = format!(
        "{}/postgres",
        config.connection_string().trim_end_matches('/')
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&admin_url).await,
        "failed to connect to maintenance database for verification",
    );

    let exists: Option<i32> = test_expect_ok(
        sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
            .bind(config.database_name())
            .fetch_optional(&mut connection)
            .await,
        "failed to check database existence",
    );

    exists.is_some()
}

async fn test_create_database(config: &MigrationConfiguration) {
    let admin_url = format!(
        "{}/postgres",
        config.connection_string().trim_end_matches('/')
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&admin_url).await,
        "failed to connect to maintenance database",
    );

    let query = format!("CREATE DATABASE \"{}\"", config.database_name());
    test_expect_ok(
        connection.execute(AssertSqlSafe(query)).await,
        "failed to create the database up front",
    );
}

async fn test_execute_in_database(config: &MigrationConfiguration, sql: &str) {
    let url = format!(
        "{}/{}",
        config.connection_string().trim_end_matches('/'),
        config.database_name()
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&url).await,
        "failed to connect for setup",
    );
    test_expect_ok(
        connection.execute(AssertSqlSafe(sql.to_string())).await,
        "failed to run setup statement",
    );
}

async fn test_set_database_accepts_connections(config: &MigrationConfiguration, accepts: bool) {
    let admin_url = format!(
        "{}/postgres",
        config.connection_string().trim_end_matches('/')
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&admin_url).await,
        "failed to connect to maintenance database",
    );

    let query = format!(
        "ALTER DATABASE \"{}\" WITH ALLOW_CONNECTIONS {accepts}",
        config.database_name()
    );
    test_expect_ok(
        connection.execute(AssertSqlSafe(query)).await,
        "failed to change whether the database accepts connections",
    );
}

async fn test_table_exists(config: &MigrationConfiguration, table_name: &str) -> bool {
    let url = format!(
        "{}/{}",
        config.connection_string().trim_end_matches('/'),
        config.database_name()
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&url).await,
        "failed to connect for verification",
    );

    let regclass: Option<String> = test_expect_ok(
        sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(table_name)
            .fetch_one(&mut connection)
            .await,
        "failed to check table existence",
    );

    regclass.is_some()
}

async fn test_user_table_names(config: &MigrationConfiguration) -> Vec<String> {
    let url = format!(
        "{}/{}",
        config.connection_string().trim_end_matches('/'),
        config.database_name()
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&url).await,
        "failed to connect for verification",
    );

    let rows = test_expect_ok(
        sqlx::query(
            "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&mut connection)
        .await,
        "failed to list user tables",
    );

    rows.into_iter().map(|row| row.get("tablename")).collect()
}

async fn test_fetch_tracking_rows(config: &MigrationConfiguration) -> Vec<TrackingRow> {
    let url = format!(
        "{}/{}",
        config.connection_string().trim_end_matches('/'),
        config.database_name()
    );
    let mut connection = test_expect_ok(
        PgConnection::connect(&url).await,
        "failed to connect for verification",
    );

    let rows = test_expect_ok(
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

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn when_nothing_goes_wrong_with_running_the_migrations_on_an_empty_database() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);

    let migrator = DbMigrator::with_clock_mock(
        FixedClockMock(executed_at),
        ["20211230_001_DoStuffScript.sql".to_string()],
    );

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");

    test_expect_ok(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to succeed",
    );

    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "script was run");
    assert_logged!("INFO", "20211230_002_Script2p.sql");
    assert_logged!("INFO", "20211231_001_Script1p.sql");
    assert_logged!("INFO", "migration process executed successfully");

    let rows = test_fetch_tracking_rows(&config).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2p.sql");
    assert_eq!(first.executed_at, executed_at);
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(second.filename, "20211231_001_Script1p.sql");
    assert_eq!(second.executed_at, executed_at);

    assert!(
        test_table_exists(&config, "customers").await,
        "expected customers table to have been created by 20211230_002_Script2p.sql"
    );
    assert!(
        test_table_exists(&config, "distributors").await,
        "expected distributors table to have been created by 20211231_001_Script1p.sql"
    );
    assert!(
        !test_table_exists(&config, "schools").await,
        "expected schools table to not exist since 20211230_001_DoStuffScript.sql was excluded"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn can_skip_scripts_if_they_already_ran_before() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let excluded = ["20211230_001_DoStuffScript.sql".to_string()];

    let executed_first_time_at = test_datetime(2021, 12, 30, 2, 16, 1);
    let migrator1 =
        DbMigrator::with_clock_mock(FixedClockMock(executed_first_time_at), excluded.clone());

    test_expect_ok(
        migrator1
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    test_expect_ok(
        migrator1
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the first migration run to succeed",
    );

    let executed_second_time_at = test_datetime(2021, 12, 31, 2, 16, 1);
    let migrator2 = DbMigrator::with_clock_mock(FixedClockMock(executed_second_time_at), excluded);

    test_expect_ok(
        migrator2
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the second migration run to succeed",
    );

    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "setup versioning table executed successfully");
    assert_logged!(
        "INFO",
        "script was not run because script was already executed"
    );
    assert_logged!("INFO", "migration process executed successfully");

    let rows = test_fetch_tracking_rows(&config).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2p.sql");
    assert_eq!(first.executed_at, executed_first_time_at);
    assert_eq!(second.filename, "20211231_001_Script1p.sql");
    assert_eq!(second.executed_at, executed_first_time_at);

    assert!(
        test_table_exists(&config, "customers").await,
        "expected customers table to have been created by 20211230_002_Script2p.sql"
    );
    assert!(
        test_table_exists(&config, "distributors").await,
        "expected distributors table to have been created by 20211231_001_Script1p.sql"
    );
    assert!(
        !test_table_exists(&config, "schools").await,
        "expected schools table to not exist since 20211230_001_DoStuffScript.sql was excluded"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn can_cancel_the_migration_process() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(
        FixedClockMock(executed_at),
        ["20211230_001_DoStuffScript.sql".to_string()],
    );

    let cancellation_token = CancellationToken::new();

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    cancellation_token.cancel();

    test_expect_ok(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &cancellation_token)
            .await,
        "expected a cancelled run to still report success",
    );

    assert_logged!("WARN", "migration process was canceled from the outside");

    assert!(
        !test_database_exists(&config).await,
        "expected database to not have been created since migration was cancelled before any setup ran"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn reports_failure_when_a_script_fails_and_stops_running_later_scripts() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_failure_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to report failure when a script fails",
    );
    assert_eq!(message, "migration process executed with errors");
    assert_logged!("ERROR", &message);

    assert_logged!("ERROR", "script was not completed due to exception");
    assert_logged!(
        "WARN",
        "script was skipped due to exception in previous script"
    );

    let rows = test_fetch_tracking_rows(&config).await;
    let [only] = rows.as_slice() else {
        panic!("expected exactly 1 tracking row for the one script that succeeded, got {rows:#?}");
    };
    assert_eq!(only.filename, "20220101_001_GoodScriptp.sql");
    assert_eq!(only.executed_at, executed_at);
    assert_eq!(only.version, env!("CARGO_PKG_VERSION"));

    let tables = test_user_table_names(&config).await;
    assert_eq!(
        tables,
        vec!["dbmigrationsrun".to_string(), "good_table".to_string()],
        "expected exactly the tracking table and good_table to exist — the broken script created nothing and the skipped script must not have run"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn reports_failure_and_runs_nothing_when_the_connection_string_is_empty() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");
    let database_name = test_random_database_name();

    let verification_config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            database_name.clone(),
            test_fixtures_dir(),
        ),
        "invalid verification configuration",
    );

    let empty_connection_config = test_expect_ok(
        MigrationConfiguration::new("   ", database_name, test_fixtures_dir()),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(
                DatabaseKind::Postgresql,
                &empty_connection_config,
                &CancellationToken::new(),
            )
            .await,
        "expected an empty connection string to report failure",
    );
    assert_eq!(message, "empty connectionstring is not valid");
    assert_logged!("ERROR", &message);
    assert!(
        !logs_contain("setup database executed successfully"),
        "expected no database setup to run for an empty connection string"
    );
    assert!(
        !logs_contain("script was run"),
        "expected no script to run for an empty connection string"
    );

    assert!(
        !test_database_exists(&verification_config).await,
        "expected the target database to not have been created since the run stopped on the empty connection string"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn reports_failure_when_the_scripts_could_not_be_loaded() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let missing_scripts_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/postgres/does_not_exist");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            missing_scripts_dir,
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to report failure when the scripts cannot be loaded",
    );
    assert_eq!(
        message,
        "One or more scripts could not be loaded, is the sequence patterns correct?"
    );
    assert_logged!("ERROR", &message);
    assert_logged!("ERROR", "migration process executed with errors");
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the scripts could not be loaded"
    );

    assert!(
        test_database_exists(&config).await,
        "expected the database to exist, since it is created before scripts are loaded"
    );

    let tables = test_user_table_names(&config).await;
    assert_eq!(
        tables,
        vec!["dbmigrationsrun".to_string()],
        "expected only the tracking table — no migration script may have created anything"
    );

    let rows = test_fetch_tracking_rows(&config).await;
    assert!(
        rows.is_empty(),
        "expected no tracking rows to have been written, got {rows:#?}"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn reports_failure_when_the_tracking_table_cannot_be_created() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    test_create_database(&config).await;
    test_set_database_accepts_connections(&config, false).await;

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to report failure when the tracking table cannot be created",
    );
    assert_eq!(message, "setup versioning table executed with errors");
    assert_logged!("ERROR", &message);
    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("ERROR", "migration process executed with errors");
    assert!(
        !logs_contain("setup versioning table executed successfully"),
        "expected the versioning-table step to not report success"
    );
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the tracking table could not be created"
    );

    test_set_database_accepts_connections(&config, true).await;

    assert!(
        !test_table_exists(&config, "dbmigrationsrun").await,
        "expected the tracking table to not exist"
    );
    let tables = test_user_table_names(&config).await;
    assert!(
        tables.is_empty(),
        "expected no tables at all — no tracking table and no script ran, got {tables:?}"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn deleting_a_database_that_does_not_exist_is_a_no_op() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    assert!(
        !test_database_exists(&config).await,
        "expected the database to not exist before the delete"
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Postgresql, &config)
            .await,
        "expected deleting a database that does not exist to be a no-op",
    );

    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync executed with error"),
        "expected no error to be logged when there was nothing to delete"
    );
    assert!(
        !test_database_exists(&config).await,
        "expected the database to still not exist afterwards"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn creating_a_database_that_already_exists_keeps_it_and_its_data() {
    let container = test_expect_ok(
        Postgres::default().start().await,
        "failed to start postgres container",
    );
    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(5432).await,
        "failed to get port",
    );
    let base_connection_string = format!("postgres://postgres:postgres@{host}:{port}");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            base_connection_string,
            test_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    test_create_database(&config).await;
    test_execute_in_database(
        &config,
        "CREATE TABLE marker_table (id integer PRIMARY KEY)",
    )
    .await;

    assert!(
        test_database_exists(&config).await,
        "expected the database to exist before the migration run"
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_apply_migrations(DatabaseKind::Postgresql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to succeed against an already existing database",
    );

    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "migration process executed successfully");
    assert!(
        !logs_contain("setup database executed with errors"),
        "expected no error for a database that was already there"
    );

    assert!(
        test_table_exists(&config, "marker_table").await,
        "expected the pre-existing table to survive — the database must not be recreated"
    );

    let rows = test_fetch_tracking_rows(&config).await;
    assert_eq!(
        rows.len(),
        3,
        "expected all three fixture scripts to be tracked, got {rows:#?}"
    );
}
