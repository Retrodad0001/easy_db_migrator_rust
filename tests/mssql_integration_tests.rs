#![allow(clippy::panic, missing_docs)]

use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use easy_db_migrator_rust::{
    CancellationToken, ClockMock, DatabaseKind, DbMigrator, MigrationConfiguration,
};
use rand::RngExt;
use testcontainers_modules::mssql_server::MssqlServer;
use testcontainers_modules::testcontainers::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tiberius::{Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use tracing_test::traced_test;

const SA_PASSWORD: &str = "yourStrong(!)Password";

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

    let is_deleted = migrator
        .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
        .await;
    assert!(is_deleted, "expected DeleteDatabaseIfExistAsync to succeed");
    assert!(logs_contain("DeleteDatabaseIfExistAsync has executed"));

    let has_succeeded = migrator
        .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
        .await;
    assert!(has_succeeded, "expected the migration run to succeed");

    assert!(logs_contain("setup database executed successfully"));
    assert!(logs_contain("script was run"));
    assert!(logs_contain("20211230_002_Script2.sql"));
    assert!(logs_contain("20211231_001_Script1.sql"));
    assert!(logs_contain("migration process executed successfully"));

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

    assert!(
        migrator1
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await
    );
    assert!(
        migrator1
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
            .await
    );

    let executed_second_time_at = test_datetime(2021, 12, 31, 2, 16, 1);
    let migrator2 = DbMigrator::with_clock_mock(FixedClockMock(executed_second_time_at), excluded);

    assert!(
        migrator2
            .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
            .await
    );

    assert!(logs_contain("setup database executed successfully"));
    assert!(logs_contain("setup versioning table executed successfully"));
    assert!(logs_contain(
        "script was not run because script was already executed"
    ));
    assert!(logs_contain("migration process executed successfully"));

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

    assert!(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await
    );

    cancellation_token.cancel();

    let succeeded = migrator
        .try_apply_migrations(DatabaseKind::Mssql, &config, &cancellation_token)
        .await;
    assert!(
        succeeded,
        "expected a cancelled run to still report success"
    );

    assert!(logs_contain(
        "migration process was canceled from the outside"
    ));

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

    assert!(
        migrator
            .try_delete_database_if_exists(DatabaseKind::Mssql, &config)
            .await
    );

    let has_succeeded = migrator
        .try_apply_migrations(DatabaseKind::Mssql, &config, &CancellationToken::new())
        .await;
    assert!(
        !has_succeeded,
        "expected the migration run to report failure when a script fails"
    );

    assert!(logs_contain("script was not completed due to exception"));
    assert!(logs_contain(
        "script was skipped due to exception in previous script"
    ));
    assert!(logs_contain("migration process executed with errors"));

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

    let has_succeeded = migrator
        .try_apply_migrations(
            DatabaseKind::Mssql,
            &empty_connection_config,
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !has_succeeded,
        "expected an empty connection string to report failure"
    );

    assert!(logs_contain("empty connectionstring is not valid"));
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
