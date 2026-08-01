#![allow(clippy::panic, missing_docs)]

use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use easy_db_migrator_rust::{
    CancellationToken, ClockMock, DatabaseKind, DbMigrator, Error, MigrationConfiguration,
};
use rand::RngExt;
use testcontainers_modules::mssql_server::MssqlServer;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tiberius::{Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tracing_test::traced_test;

const SA_PASSWORD: &str = "yourStrong(!)Password";

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

fn test_expect_some_value<T>(value: Option<T>, context: &str) -> T {
    match value {
        Some(value) => value,
        None => panic!("{context}"),
    }
}

fn test_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mssql/test_scripts")
}

fn test_failure_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mssql/test_scripts_failure")
}

fn test_get_random_database_name() -> String {
    let suffix: u32 = rand::rng().random_range(100_000..999_999);
    format!("testmssql{suffix}")
}

fn test_datetime(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> DateTime<Utc> {
    test_expect_some_value(
        Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
            .single(),
        "not a valid unambiguous UTC datetime",
    )
}

async fn test_database_exists(config: &MigrationConfiguration) -> bool {
    let tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );

    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for verification",
    );
    let mut client = test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for verification",
    );

    let database_name = config.database_name();
    let row = test_expect_ok(
        test_expect_ok(
            client
                .query(
                    "SELECT 1 FROM sys.databases WHERE name = @P1",
                    &[&database_name],
                )
                .await,
            "failed to check database existence",
        )
        .into_row()
        .await,
        "failed to collect database-existence row",
    );

    row.is_some()
}

async fn test_connect_without_database(
    config: &MigrationConfiguration,
) -> Client<Compat<TcpStream>> {
    let tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );
    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for setup",
    );
    test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for setup",
    )
}

async fn test_create_database(config: &MigrationConfiguration) {
    let mut client = test_connect_without_database(config).await;
    let query = format!("CREATE DATABASE [{}]", config.database_name());
    test_expect_ok(
        client.execute(query.as_str(), &[]).await,
        "failed to create the database up front",
    );
}

async fn test_execute_in_database(config: &MigrationConfiguration, sql: &str) {
    let mut tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );
    tds_config.database(config.database_name());

    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for setup",
    );
    let mut client = test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for setup",
    );

    test_expect_ok(
        client.execute(sql, &[]).await,
        "failed to run setup statement",
    );
}

async fn test_set_database_online(config: &MigrationConfiguration, online: bool) {
    let mut client = test_connect_without_database(config).await;
    let database_name = config.database_name();
    let query = if online {
        format!("ALTER DATABASE [{database_name}] SET ONLINE")
    } else {
        format!("ALTER DATABASE [{database_name}] SET OFFLINE WITH ROLLBACK IMMEDIATE")
    };
    test_expect_ok(
        client.execute(query.as_str(), &[]).await,
        "failed to change whether the database is online",
    );
}

async fn test_table_exists(config: &MigrationConfiguration, table_name: &str) -> bool {
    let mut tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );

    tds_config.database(config.database_name());

    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for verification",
    );
    let mut client = test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for verification",
    );

    let row = test_expect_ok(
        test_expect_ok(
            client
                .query(
                    "SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'",
                    &[&table_name],
                )
                .await,
            "failed to check table existence",
        )
        .into_row()
        .await,
        "failed to collect table-existence row",
    );

    row.is_some()
}

async fn test_user_table_names(config: &MigrationConfiguration) -> Vec<String> {
    let mut tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );

    tds_config.database(config.database_name());

    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for verification",
    );
    let mut client = test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for verification",
    );

    let rows = test_expect_ok(
        test_expect_ok(
            client
                .query("SELECT name FROM sys.tables ORDER BY name", &[])
                .await,
            "failed to list user tables",
        )
        .into_first_result()
        .await,
        "failed to collect user table rows",
    );

    let mut names = Vec::with_capacity(rows.len());
    for row in rows {
        let name: &str = test_expect_some_value(row.get("name"), "missing name column");
        names.push(name.to_string());
    }
    names
}

async fn test_fetch_tracking_rows(config: &MigrationConfiguration) -> Vec<TrackingRow> {
    let mut tds_config = test_expect_ok(
        Config::from_ado_string(config.connection_string()),
        "invalid ado connection string",
    );

    tds_config.database(config.database_name());

    let tcp = test_expect_ok(
        TcpStream::connect(tds_config.get_addr()).await,
        "failed to connect for verification",
    );
    let mut client = test_expect_ok(
        Client::connect(tds_config, tcp.compat_write()).await,
        "failed to establish tds connection for verification",
    );

    let rows = test_expect_ok(
        test_expect_ok(
            client
                .query(
                    "SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id",
                    &[],
                )
                .await,
            "failed to query tracking table",
        )
        .into_first_result()
        .await,
        "failed to collect tracking rows",
    );

    let mut tracking_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let filename: &str = test_expect_some_value(row.get("Filename"), "missing Filename column");
        let executed_at: DateTime<Utc> =
            test_expect_some_value(row.get("ExecutedAt"), "missing ExecutedAt column");
        let version: &str = test_expect_some_value(row.get("Version"), "missing Version column");
        tracking_rows.push(TrackingRow {
            filename: filename.to_string(),
            executed_at,
            version: version.to_string(),
        });
    }

    tracking_rows
}

async fn test_start_mssql() -> (ContainerAsync<MssqlServer>, String) {
    let container = test_expect_ok(
        MssqlServer::default()
            .with_accept_eula()
            .with_sa_password(SA_PASSWORD)
            .start()
            .await,
        "failed to start mssql container",
    );

    let host = test_expect_ok(container.get_host().await, "failed to get host");
    let port = test_expect_ok(
        container.get_host_port_ipv4(1433).await,
        "failed to get port",
    );

    let connection_string = format!(
        "Server=tcp:{host},{port};User Id=sa;Password={SA_PASSWORD};TrustServerCertificate=True;Encrypt=False;"
    );

    (container, connection_string)
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn when_nothing_goes_wrong_with_running_the_migrations_on_an_empty_database() {
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(
        FixedClockMock(executed_at),
        ["20211230_001_CreateDB.sql".to_string()],
    );

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");

    test_expect_ok(
        migrator
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
            .await,
        "expected the migration run to succeed",
    );

    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "script was run");
    assert_logged!("INFO", "20211230_002_Script2.sql");
    assert_logged!("INFO", "20211231_001_Script1.sql");
    assert_logged!("INFO", "migration process executed successfully");

    let tracked_rows = test_fetch_tracking_rows(&config).await;
    let [first, second] = tracked_rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {tracked_rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2.sql");
    assert_eq!(first.executed_at, executed_at);
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(second.filename, "20211231_001_Script1.sql");
    assert_eq!(second.executed_at, executed_at);

    assert!(
        test_table_exists(&config, "bb").await,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    assert!(
        test_table_exists(&config, "aa").await,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    assert!(
        !test_table_exists(&config, "placeholder").await,
        "expected table placeholder to not exist since 20211230_001_CreateDB.sql was excluded"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn can_skip_scripts_if_they_already_ran_before() {
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let excluded = ["20211230_001_CreateDB.sql".to_string()];

    let executed_first_time_at = test_datetime(2021, 12, 30, 2, 16, 1);
    let migrator1 =
        DbMigrator::with_clock_mock(FixedClockMock(executed_first_time_at), excluded.clone());

    test_expect_ok(
        migrator1
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    test_expect_ok(
        migrator1
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
            .await,
        "expected the first migration run to succeed",
    );

    let executed_second_time_at = test_datetime(2021, 12, 31, 2, 16, 1);
    let migrator2 = DbMigrator::with_clock_mock(FixedClockMock(executed_second_time_at), excluded);

    test_expect_ok(
        migrator2
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
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
    assert_eq!(first.filename, "20211230_002_Script2.sql");
    assert_eq!(first.executed_at, executed_first_time_at);
    assert_eq!(second.filename, "20211231_001_Script1.sql");
    assert_eq!(second.executed_at, executed_first_time_at);

    assert!(
        test_table_exists(&config, "bb").await,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    assert!(
        test_table_exists(&config, "aa").await,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    assert!(
        !test_table_exists(&config, "placeholder").await,
        "expected table placeholder to not exist since 20211230_001_CreateDB.sql was excluded"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn can_cancel_the_migration_process() {
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(
        FixedClockMock(executed_at),
        ["20211230_001_CreateDB.sql".to_string()],
    );

    let cancellation_token = CancellationToken::new();

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    cancellation_token.cancel();

    test_expect_ok(
        migrator
            .try_apply_migrations(DatabaseKind::Mssql, &config, &cancellation_token)
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
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_failure_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
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
    assert_eq!(only.filename, "20220101_001_GoodScript.sql");
    assert_eq!(only.executed_at, executed_at);
    assert_eq!(only.version, env!("CARGO_PKG_VERSION"));

    let tables = test_user_table_names(&config).await;
    assert_eq!(
        tables,
        vec!["DbMigrationsRun".to_string(), "goodtable".to_string()],
        "expected exactly the tracking table and goodtable to exist — the broken script created nothing and the skipped script must not have run"
    );
}

#[cfg(test)]
#[tokio::test]
#[traced_test]
async fn reports_failure_and_runs_nothing_when_the_connection_string_is_empty() {
    let (_container, connection_string) = test_start_mssql().await;
    let database_name = test_get_random_database_name();

    let verification_config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
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
                DatabaseKind::Mssql,
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
    let (_container, connection_string) = test_start_mssql().await;

    let missing_scripts_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mssql/does_not_exist");

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            missing_scripts_dir,
        ),
        "invalid migration configuration",
    );

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    test_expect_ok(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
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
        vec!["DbMigrationsRun".to_string()],
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
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    test_create_database(&config).await;
    test_set_database_online(&config, false).await;

    let executed_at = test_datetime(2021, 10, 17, 12, 10, 10);
    let migrator = DbMigrator::with_clock_mock(FixedClockMock(executed_at), Vec::<String>::new());

    let message = test_expect_migration_error(
        migrator
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
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

    test_set_database_online(&config, true).await;

    assert!(
        !test_table_exists(&config, "DbMigrationsRun").await,
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
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
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
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
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
    let (_container, connection_string) = test_start_mssql().await;

    let config = test_expect_ok(
        MigrationConfiguration::new(
            connection_string,
            test_get_random_database_name(),
            test_fixtures_dir(),
        ),
        "invalid migration configuration",
    );

    test_create_database(&config).await;
    test_execute_in_database(
        &config,
        "CREATE TABLE marker_table (Id int IDENTITY(1,1) PRIMARY KEY)",
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
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
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
