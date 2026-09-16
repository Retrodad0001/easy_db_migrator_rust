#![allow(clippy::panic, missing_docs)]

mod test_support;

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use easy_db_migrator_rust::{
    DatabaseKind, MigrationConfiguration, try_apply_migrations, try_delete_database_if_exists,
};
use tracing_test::traced_test;

use test_support::{
    assert_logged, determine_a_unique_database_name, expect_migration_error, expect_ok,
};

#[cfg(feature = "postgres")]
use test_support::postgres;

#[cfg(feature = "mssql")]
use test_support::mssql;

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_empty_postgres_database_when_migrations_run_then_every_script_is_applied_and_tracked()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name, the postgres
    // fixture scripts, and 20211230_001_DoStuffScript.sql excluded. The database
    // does not exist before the delete. Deleting the database succeeds, logs
    // "DeleteDatabaseIfExistAsync has executed" at INFO, and leaves no database,
    // which is the state the run starts from. The migration run succeeds and
    // logs "setup database executed
    // successfully", "script was run", "20211230_002_Script2p.sql",
    // "20211231_001_Script1p.sql" and "migration process executed successfully" at
    // INFO. The tracking table holds exactly two rows: 20211230_002_Script2p.sql
    // first and 20211231_001_Script1p.sql second. Each row has an executed_at
    // between the time before and the time after the run, and the first row has
    // the crate version. The customers and distributors tables exist, and the
    // schools table does not.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_DoStuffScript.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed",
    );
    let after = Utc::now();
    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "script was run");
    assert_logged!("INFO", "20211230_002_Script2p.sql");
    assert_logged!("INFO", "20211231_001_Script1p.sql");
    assert_logged!("INFO", "migration process executed successfully");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2p.sql");
    assert!(before <= first.executed_at && first.executed_at <= after);
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(second.filename, "20211231_001_Script1p.sql");
    assert!(before <= second.executed_at && second.executed_at <= after);
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "customers").await,
        "expected customers table to have been created by 20211230_002_Script2p.sql"
    );
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "distributors").await,
        "expected distributors table to have been created by 20211231_001_Script1p.sql"
    );
    assert!(
        !postgres::is_table_existing(&connection_string, &database_name, "schools").await,
        "expected schools table to not exist since 20211230_001_DoStuffScript.sql was excluded"
    );
    eprintln!("[5/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_scripts_already_applied_when_migrations_run_again_then_they_are_skipped()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name, the postgres
    // fixture scripts, and 20211230_001_DoStuffScript.sql excluded. The database
    // does not exist before the delete. The delete succeeds, and the database
    // still does not exist after it. The migrations run once and succeed, and
    // the tracking table then holds 20211230_002_Script2p.sql and
    // 20211231_001_Script1p.sql, in that order. The migrations then run a second
    // time. The second run succeeds, and it
    // logs "setup database executed successfully", "setup versioning table executed
    // successfully", "script was not run because script was already executed" and
    // "migration process executed successfully" at INFO. The tracking table holds
    // exactly two rows, 20211230_002_Script2p.sql and 20211231_001_Script1p.sql, and
    // both have an executed_at from the first run, not from the second. The customers
    // and distributors tables exist, and the schools table does not.
    const TOTAL: u8 = 6;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_DoStuffScript.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the first run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations the first time");
    let first_run_started_at = Utc::now();
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the first migration run to succeed",
    );
    let first_run_finished_at = Utc::now();
    let rows_after_the_first_run =
        postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames_after_the_first_run: Vec<&str> = rows_after_the_first_run
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames_after_the_first_run,
        ["20211230_002_Script2p.sql", "20211231_001_Script1p.sql"],
        "expected the first run to track both scripts, before the second run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations the first time - passed");

    eprintln!("[4/{TOTAL}] BEGIN running the migrations the second time");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    eprintln!("[4/{TOTAL}] END running the migrations the second time - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2p.sql");
    assert!(first_run_started_at <= first.executed_at);
    assert!(first.executed_at <= first_run_finished_at);
    assert_eq!(second.filename, "20211231_001_Script1p.sql");
    assert!(first_run_started_at <= second.executed_at);
    assert!(second.executed_at <= first_run_finished_at);
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[6/{TOTAL}] BEGIN checking the tables");
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "customers").await,
        "expected customers table to have been created by 20211230_002_Script2p.sql"
    );
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "distributors").await,
        "expected distributors table to have been created by 20211231_001_Script1p.sql"
    );
    assert!(
        !postgres::is_table_existing(&connection_string, &database_name, "schools").await,
        "expected schools table to not exist since 20211230_001_DoStuffScript.sql was excluded"
    );
    eprintln!("[6/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_cancelled_flag_when_postgres_migrations_run_then_the_run_stops() {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and the
    // postgres fixture scripts. The database does not exist before the delete.
    // The delete succeeds, and the database still does not exist after it. The
    // cancel flag is set to true, and the migrations run. The run still returns success, logs "migration
    // process was canceled from the outside" at WARN, and creates no database.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_DoStuffScript.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database and setting the cancel flag");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    let is_cancelled = AtomicBool::new(false);
    is_cancelled.store(true, Ordering::SeqCst);
    eprintln!("[2/{TOTAL}] END deleting the database and setting the cancel flag - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &is_cancelled,
        )
        .await,
        "expected a cancelled run to still report success",
    );
    assert_logged!("WARN", "migration process was canceled from the outside");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected database to not have been created since migration was cancelled before any setup ran"
    );
    eprintln!("[4/{TOTAL}] END checking the database - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_failing_postgres_script_when_migrations_run_then_failure_is_reported_and_later_scripts_do_not_run()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and the postgres
    // failure fixture scripts: a good script, a broken script and a later script.
    // The database does not exist before the delete. The delete succeeds, and the
    // database still does not exist after it. The migrations run. The run fails
    // with "migration
    // process executed with errors", logged at ERROR, and logs "script was not
    // completed due to exception" at ERROR and "script was skipped due to exception
    // in previous script" at WARN. The tracking table holds exactly one row,
    // 20220101_001_GoodScriptp.sql, with an executed_at between the time before and
    // the time after the run and the crate version. The only tables are
    // dbmigrationsrun and good_table.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts_failure"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to report failure when a script fails",
    );
    let after = Utc::now();
    assert_eq!(message, "migration process executed with errors");
    assert_logged!("ERROR", &message);
    assert_logged!("ERROR", "script was not completed due to exception");
    assert_logged!(
        "WARN",
        "script was skipped due to exception in previous script"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let [only] = rows.as_slice() else {
        panic!("expected exactly 1 tracking row for the one script that succeeded, got {rows:#?}");
    };
    assert_eq!(only.filename, "20220101_001_GoodScriptp.sql");
    assert!(before <= only.executed_at && only.executed_at <= after);
    assert_eq!(only.version, env!("CARGO_PKG_VERSION"));
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    let tables = postgres::read_the_user_table_names(&connection_string, &database_name).await;
    assert_eq!(
        tables,
        vec!["dbmigrationsrun".to_string(), "good_table".to_string()],
        "expected exactly the tracking table and good_table to exist — the broken script created nothing and the skipped script must not have run"
    );
    eprintln!("[5/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_scripts_that_cannot_be_loaded_when_migrations_run_then_failure_is_reported()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and a scripts
    // directory that does not exist. The database does not exist before the
    // delete. The delete succeeds, and the database still does not exist after
    // it. The migrations run.
    // The run fails with "One or more scripts could not be loaded, is the
    // sequence patterns correct?", logged at ERROR, logs "migration process
    // executed with errors" at ERROR, and logs no "script was run". The database
    // exists, its only table is dbmigrationsrun, and the tracking table is empty.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("does_not_exist"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and its tables");
    assert!(
        postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist, since it is created before scripts are loaded"
    );
    let tables = postgres::read_the_user_table_names(&connection_string, &database_name).await;
    assert_eq!(
        tables,
        vec!["dbmigrationsrun".to_string()],
        "expected only the tracking table — no migration script may have created anything"
    );
    eprintln!("[4/{TOTAL}] END checking the database and its tables - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    assert!(
        rows.is_empty(),
        "expected no tracking rows to have been written, got {rows:#?}"
    );
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_database_refuses_connections_when_migrations_run_then_tracking_table_failure_is_reported()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and the
    // postgres fixture scripts. The database is created up front and set to refuse
    // connections. Before the run, the database exists and refuses connections.
    // The migrations run. The run fails with "setup versioning
    // table executed with errors", logged at ERROR, logs "setup database executed
    // successfully" at INFO and "migration process executed with errors" at ERROR,
    // and logs neither "setup versioning table executed successfully" nor "script
    // was run". The database still refuses connections after the run. After the
    // database accepts connections again, dbmigrationsrun does not exist and the
    // database has no tables at all.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating a database that refuses connections");
    postgres::create_the_database(&connection_string, &database_name).await;
    postgres::set_whether_the_database_accepts_connections(
        &connection_string,
        &database_name,
        false,
    )
    .await;
    assert!(
        postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist before the run"
    );
    assert!(
        !postgres::is_database_accepting_connections(&connection_string, &database_name).await,
        "expected the database to refuse connections before the run"
    );
    eprintln!("[2/{TOTAL}] END creating a database that refuses connections - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    assert!(
        !postgres::is_database_accepting_connections(&connection_string, &database_name).await,
        "expected the database to still refuse connections after the run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    postgres::set_whether_the_database_accepts_connections(
        &connection_string,
        &database_name,
        true,
    )
    .await;
    assert!(
        !postgres::is_table_existing(&connection_string, &database_name, "dbmigrationsrun").await,
        "expected the tracking table to not exist"
    );
    let tables = postgres::read_the_user_table_names(&connection_string, &database_name).await;
    assert!(
        tables.is_empty(),
        "expected no tables at all — no tracking table and no script ran, got {tables:?}"
    );
    eprintln!("[4/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_no_postgres_database_when_delete_runs_then_nothing_changes() {
    // What it asserts.
    // A new PostgreSQL container gets a database name that no database has. The
    // database does not exist before the delete. The delete succeeds, logs
    // "DeleteDatabaseIfExistAsync has executed" at INFO, and logs no
    // "DeleteDatabaseIfExistAsync executed with error". The database still does
    // not exist afterwards.
    const TOTAL: u8 = 3;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected deleting a database that does not exist to be a no-op",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync executed with error"),
        "expected no error to be logged when there was nothing to delete"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN checking the database");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to still not exist afterwards"
    );
    eprintln!("[3/{TOTAL}] END checking the database - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_database_with_data_when_migrations_run_then_its_data_is_kept() {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name, created up
    // front with a marker_table in it, and the postgres fixture scripts. Before
    // the run, the database and marker_table exist, and dbmigrationsrun does not.
    // The migrations run and succeed, log "setup database executed successfully" and
    // "migration process executed successfully" at INFO, and log no "setup
    // database executed with errors". The marker_table still exists, and the
    // tracking table holds three rows: 20211230_001_DoStuffScript.sql,
    // 20211230_002_Script2p.sql and 20211231_001_Script1p.sql, in that order.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = determine_a_unique_database_name("testpostgres");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating the database with a marker table");
    postgres::create_the_database(&connection_string, &database_name).await;
    postgres::execute_in_the_database(
        &connection_string,
        &database_name,
        "CREATE TABLE marker_table (id integer PRIMARY KEY)",
    )
    .await;
    assert!(
        postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist before the migration run"
    );
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "marker_table").await,
        "expected the marker table to exist before the run"
    );
    assert!(
        !postgres::is_table_existing(&connection_string, &database_name, "dbmigrationsrun").await,
        "expected no tracking table before the run"
    );
    eprintln!("[2/{TOTAL}] END creating the database with a marker table - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed against an already existing database",
    );
    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "migration process executed successfully");
    assert!(
        !logs_contain("setup database executed with errors"),
        "expected no error for a database that was already there"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the marker table and the tracking table");
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "marker_table").await,
        "expected the pre-existing table to survive — the database must not be recreated"
    );
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        [
            "20211230_001_DoStuffScript.sql",
            "20211230_002_Script2p.sql",
            "20211231_001_Script1p.sql",
        ],
        "expected all three fixture scripts to be tracked in order, got {rows:#?}"
    );
    eprintln!("[4/{TOTAL}] END checking the marker table and the tracking table - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_connection_string_with_a_database_and_a_query_when_migrations_run_then_every_script_is_applied()
 {
    // What it asserts.
    // A new PostgreSQL container gets a connection string that ends in
    // /postgres?sslmode=disable, a database with a unique name, and the
    // postgres fixture scripts with 20211230_001_DoStuffScript.sql excluded.
    // The database does not exist before the delete. The delete succeeds,
    // and the database still does not exist after it. The migrations run
    // and succeed, and log "migration process executed successfully" at
    // INFO. The tracking table holds 20211230_002_Script2p.sql and
    // 20211231_001_Script1p.sql, in that order.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let database_name = determine_a_unique_database_name("testpostgres");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            format!("{connection_string}/postgres?sslmode=disable"),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_DoStuffScript.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected the delete to succeed with a database and a query in the connection string",
    );
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed with a database and a query in the connection string",
    );
    assert_logged!("INFO", "migration process executed successfully");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20211230_002_Script2p.sql", "20211231_001_Script1p.sql"],
        "expected both scripts to be tracked in order, got {rows:#?}"
    );
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_server_that_does_not_answer_when_delete_runs_then_failure_is_reported()
{
    // What it asserts.
    // No server answers on 127.0.0.1 port 1 before the delete. The
    // connection string points at that port. The delete fails with
    // "DeleteDatabaseIfExistAsync executed with error", logged at ERROR, and
    // logs no "DeleteDatabaseIfExistAsync has executed".
    const TOTAL: u8 = 2;

    eprintln!("[1/{TOTAL}] BEGIN checking that no server answers on 127.0.0.1:1");
    assert!(
        std::net::TcpStream::connect("127.0.0.1:1").is_err(),
        "expected no server to answer on 127.0.0.1:1"
    );
    eprintln!("[1/{TOTAL}] END checking that no server answers on 127.0.0.1:1 - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            "postgres://postgres:postgres@127.0.0.1:1",
            "testpostgres",
            postgres::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    let message = expect_migration_error(
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await,
        "expected the delete to fail when no server answers",
    );
    assert_eq!(message, "DeleteDatabaseIfExistAsync executed with error");
    assert_logged!("ERROR", &message);
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync has executed"),
        "expected no success line when the delete failed"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_server_that_does_not_answer_when_migrations_run_then_setup_failure_is_reported()
 {
    // What it asserts.
    // No server answers on 127.0.0.1 port 1 before the run. The connection
    // string points at that port. The migrations run. The run fails with
    // "setup database executed with errors", logged at ERROR, logs
    // "migration process executed with errors" at ERROR, and logs neither
    // "setup database executed successfully" nor "script was run".
    const TOTAL: u8 = 2;

    eprintln!("[1/{TOTAL}] BEGIN checking that no server answers on 127.0.0.1:1");
    assert!(
        std::net::TcpStream::connect("127.0.0.1:1").is_err(),
        "expected no server to answer on 127.0.0.1:1"
    );
    eprintln!("[1/{TOTAL}] END checking that no server answers on 127.0.0.1:1 - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            "postgres://postgres:postgres@127.0.0.1:1",
            "testpostgres",
            postgres::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the run to fail when no server answers",
    );
    assert_eq!(message, "setup database executed with errors");
    assert_logged!("ERROR", &message);
    assert_logged!("ERROR", "migration process executed with errors");
    assert!(
        !logs_contain("setup database executed successfully"),
        "expected no success line for the database set-up"
    );
    assert!(!logs_contain("script was run"), "expected no script to run");
    eprintln!("[2/{TOTAL}] END running the migrations - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_scripts_directory_with_a_folder_when_migrations_run_then_the_folder_is_skipped()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and a
    // scripts directory that holds one script and the folder notes. The
    // database does not exist before the run. The migrations run and
    // succeed, and log "migration process executed successfully" at INFO.
    // The tracking table holds exactly 20220401_001_FolderScriptp.sql, and
    // folder_table exists.
    const TOTAL: u8 = 3;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations");
    let database_name = determine_a_unique_database_name("testpostgres");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the run"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts_with_folder"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the run to succeed and skip the folder",
    );
    assert_logged!("INFO", "migration process executed successfully");
    eprintln!("[2/{TOTAL}] END running the migrations - passed");

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table and the tables");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220401_001_FolderScriptp.sql"],
        "expected only the script to be tracked, not the folder, got {rows:#?}"
    );
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "folder_table").await,
        "expected folder_table to have been created by the script"
    );
    eprintln!("[3/{TOTAL}] END reading the tracking table and the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_slow_first_postgres_script_when_the_run_is_cancelled_during_it_then_later_scripts_do_not_run()
 {
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and the
    // cancel fixtures: a slow first script and a later script. The database
    // does not exist before the run. The migrations run, and the cancel flag
    // is set while PostgreSQL runs the pg_sleep of the first script. The run
    // returns success, logs "migration process was canceled" at WARN, and
    // logs no "canceled from the outside". The tracking table holds exactly
    // 20220201_001_SlowScriptp.sql. slow_table exists, and later_table does
    // not.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations and cancelling during the first script");
    let database_name = determine_a_unique_database_name("testpostgres");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the run"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts_cancel"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    let is_cancelled = AtomicBool::new(false);
    let run = try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &is_cancelled,
    );
    let cancel = async {
        let mut tries = 0_u32;
        while !postgres::is_query_running(&connection_string, "pg_sleep").await {
            tries += 1;
            assert!(tries < 200, "expected the slow script to start within 10 s");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        is_cancelled.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(run, cancel);
    expect_ok(
        result,
        "expected a run cancelled between scripts to report success",
    );
    assert_logged!("WARN", "migration process was canceled");
    assert!(
        !logs_contain("canceled from the outside"),
        "expected the cancel between scripts, not before the start"
    );
    eprintln!(
        "[2/{TOTAL}] END running the migrations and cancelling during the first script - passed"
    );

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220201_001_SlowScriptp.sql"],
        "expected only the slow script to be tracked, got {rows:#?}"
    );
    eprintln!("[3/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    assert!(
        postgres::is_table_existing(&connection_string, &database_name, "slow_table").await,
        "expected slow_table to have been created by the slow script"
    );
    assert!(
        !postgres::is_table_existing(&connection_string, &database_name, "later_table").await,
        "expected later_table to not exist, because the run stopped before the later script"
    );
    eprintln!("[4/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[traced_test]
async fn test_given_postgres_script_that_is_not_utf8_when_migrations_run_then_failure_is_reported()
{
    // What it asserts.
    // A new PostgreSQL container gets a database with a unique name and a
    // scripts directory whose one script holds the Latin-1 byte 0xE9. The
    // database does not exist before the run. The migrations run. The run
    // fails with "One or more scripts could not be loaded, is the sequence
    // patterns correct?", logged at ERROR, and logs no "script was run". The
    // tracking table is empty.
    const TOTAL: u8 = 3;

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations");
    let database_name = determine_a_unique_database_name("testpostgres");
    assert!(
        !postgres::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the run"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            postgres::determine_the_fixtures_path("test_scripts_not_utf8"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Postgresql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the run to fail when a script is not UTF-8",
    );
    assert_eq!(
        message,
        "One or more scripts could not be loaded, is the sequence patterns correct?"
    );
    assert_logged!("ERROR", &message);
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when a script could not be read"
    );
    eprintln!("[2/{TOTAL}] END running the migrations - passed");

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = postgres::read_the_tracking_rows(&connection_string, &database_name).await;
    assert!(rows.is_empty(), "expected no tracking rows, got {rows:#?}");
    eprintln!("[3/{TOTAL}] END reading the tracking table - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_empty_mssql_database_when_migrations_run_then_every_script_is_applied_and_tracked()
 {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name, the mssql
    // fixture scripts, and 20211230_001_CreateDB.sql excluded. The database does
    // not exist before the delete. Deleting the database succeeds, logs
    // "DeleteDatabaseIfExistAsync has executed" at INFO, and leaves no database,
    // which is the state the run starts from. The migration run succeeds and
    // logs "setup database executed
    // successfully", "script was run", "20211230_002_Script2.sql",
    // "20211231_001_Script1.sql" and "migration process executed successfully" at
    // INFO. The tracking table holds exactly two rows: 20211230_002_Script2.sql
    // first and 20211231_001_Script1.sql second. Each row has an executed_at
    // between the time before and the time after the run, and the first row has
    // the crate version. The tables bb and aa exist, and placeholder does not.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_CreateDB.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed",
    );
    let after = Utc::now();
    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "script was run");
    assert_logged!("INFO", "20211230_002_Script2.sql");
    assert_logged!("INFO", "20211231_001_Script1.sql");
    assert_logged!("INFO", "migration process executed successfully");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2.sql");
    assert!(before <= first.executed_at && first.executed_at <= after);
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(second.filename, "20211231_001_Script1.sql");
    assert!(before <= second.executed_at && second.executed_at <= after);
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "bb").await,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "aa").await,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    assert!(
        !mssql::is_table_existing(&connection_string, &database_name, "placeholder").await,
        "expected table placeholder to not exist since 20211230_001_CreateDB.sql was excluded"
    );
    eprintln!("[5/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_mssql_scripts_already_applied_when_migrations_run_again_then_they_are_skipped()
{
    // What it asserts.
    // A new SQL Server container gets a database with a unique name, the mssql
    // fixture scripts, and 20211230_001_CreateDB.sql excluded. The database
    // does not exist before the delete. The delete succeeds, and the database
    // still does not exist after it. The migrations run once and succeed, and
    // the tracking table then holds 20211230_002_Script2.sql and
    // 20211231_001_Script1.sql, in that order. The migrations then run a second
    // time. The second run succeeds, and it
    // logs "setup database executed successfully", "setup versioning table executed
    // successfully", "script was not run because script was already executed" and
    // "migration process executed successfully" at INFO. The tracking table holds
    // exactly two rows, 20211230_002_Script2.sql and 20211231_001_Script1.sql, and
    // both have an executed_at from the first run, not from the second. The tables
    // bb and aa exist, and placeholder does not.
    const TOTAL: u8 = 6;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_CreateDB.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the first run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations the first time");
    let first_run_started_at = Utc::now();
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the first migration run to succeed",
    );
    let first_run_finished_at = Utc::now();
    let rows_after_the_first_run =
        mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames_after_the_first_run: Vec<&str> = rows_after_the_first_run
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames_after_the_first_run,
        ["20211230_002_Script2.sql", "20211231_001_Script1.sql"],
        "expected the first run to track both scripts, before the second run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations the first time - passed");

    eprintln!("[4/{TOTAL}] BEGIN running the migrations the second time");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    eprintln!("[4/{TOTAL}] END running the migrations the second time - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(first.filename, "20211230_002_Script2.sql");
    assert!(first_run_started_at <= first.executed_at);
    assert!(first.executed_at <= first_run_finished_at);
    assert_eq!(second.filename, "20211231_001_Script1.sql");
    assert!(first_run_started_at <= second.executed_at);
    assert!(second.executed_at <= first_run_finished_at);
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[6/{TOTAL}] BEGIN checking the tables");
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "bb").await,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "aa").await,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    assert!(
        !mssql::is_table_existing(&connection_string, &database_name, "placeholder").await,
        "expected table placeholder to not exist since 20211230_001_CreateDB.sql was excluded"
    );
    eprintln!("[6/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_cancelled_flag_when_mssql_migrations_run_then_the_run_stops() {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name and the mssql
    // fixture scripts. The database does not exist before the delete. The
    // delete succeeds, and the database still does not exist after it. The
    // cancel flag is set to true, and the migrations run. The run still returns success, logs "migration
    // process was canceled from the outside" at WARN, and creates no database.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_CreateDB.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database and setting the cancel flag");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    let is_cancelled = AtomicBool::new(false);
    is_cancelled.store(true, Ordering::SeqCst);
    eprintln!("[2/{TOTAL}] END deleting the database and setting the cancel flag - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(DatabaseKind::Mssql, &migration_configuration, &is_cancelled).await,
        "expected a cancelled run to still report success",
    );
    assert_logged!("WARN", "migration process was canceled from the outside");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected database to not have been created since migration was cancelled before any setup ran"
    );
    eprintln!("[4/{TOTAL}] END checking the database - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_failing_mssql_script_when_migrations_run_then_failure_is_reported_and_later_scripts_do_not_run()
 {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name and the mssql
    // failure fixture scripts: a good script, a broken script and a later script.
    // The database does not exist before the delete. The delete succeeds, and the
    // database still does not exist after it. The migrations run. The run fails
    // with "migration
    // process executed with errors", logged at ERROR, and logs "script was not
    // completed due to exception" at ERROR and "script was skipped due to exception
    // in previous script" at WARN. The tracking table holds exactly one row,
    // 20220101_001_GoodScript.sql, with an executed_at between the time before and
    // the time after the run and the crate version. The only tables are
    // DbMigrationsRun and goodtable.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts_failure"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to report failure when a script fails",
    );
    let after = Utc::now();
    assert_eq!(message, "migration process executed with errors");
    assert_logged!("ERROR", &message);
    assert_logged!("ERROR", "script was not completed due to exception");
    assert_logged!(
        "WARN",
        "script was skipped due to exception in previous script"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let [only] = rows.as_slice() else {
        panic!("expected exactly 1 tracking row for the one script that succeeded, got {rows:#?}");
    };
    assert_eq!(only.filename, "20220101_001_GoodScript.sql");
    assert!(before <= only.executed_at && only.executed_at <= after);
    assert_eq!(only.version, env!("CARGO_PKG_VERSION"));
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    let tables = mssql::read_the_user_table_names(&connection_string, &database_name).await;
    assert_eq!(
        tables,
        vec!["DbMigrationsRun".to_string(), "goodtable".to_string()],
        "expected exactly the tracking table and goodtable to exist — the broken script created nothing and the skipped script must not have run"
    );
    eprintln!("[5/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_mssql_scripts_that_cannot_be_loaded_when_migrations_run_then_failure_is_reported()
 {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name and a scripts
    // directory that does not exist. The database does not exist before the
    // delete. The delete succeeds, and the database still does not exist after
    // it. The migrations run.
    // The run fails with "One or more scripts could not be loaded, is the
    // sequence patterns correct?", logged at ERROR, logs "migration process
    // executed with errors" at ERROR, and logs no "script was run". The database
    // exists, its only table is DbMigrationsRun, and the tracking table is empty.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("does_not_exist"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected DeleteDatabaseIfExistAsync to succeed",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and its tables");
    assert!(
        mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist, since it is created before scripts are loaded"
    );
    let tables = mssql::read_the_user_table_names(&connection_string, &database_name).await;
    assert_eq!(
        tables,
        vec!["DbMigrationsRun".to_string()],
        "expected only the tracking table — no migration script may have created anything"
    );
    eprintln!("[4/{TOTAL}] END checking the database and its tables - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    assert!(
        rows.is_empty(),
        "expected no tracking rows to have been written, got {rows:#?}"
    );
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_mssql_database_is_offline_when_migrations_run_then_tracking_table_failure_is_reported()
 {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name and the mssql
    // fixture scripts. The database is created up front and set offline. Before
    // the run, the database exists and is offline. The migrations run. The run
    // fails with "setup versioning table executed with
    // errors", logged at ERROR, logs "setup database executed successfully" at
    // INFO and "migration process executed with errors" at ERROR, and logs neither
    // "setup versioning table executed successfully" nor "script was run". The
    // database is still offline after the run. After the database is online
    // again, DbMigrationsRun does not exist and the database has no tables at all.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating a database that is offline");
    mssql::create_the_database(&connection_string, &database_name).await;
    mssql::set_whether_the_database_is_online(&connection_string, &database_name, false).await;
    assert!(
        mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist before the run"
    );
    assert!(
        !mssql::is_database_online(&connection_string, &database_name).await,
        "expected the database to be offline before the run"
    );
    eprintln!("[2/{TOTAL}] END creating a database that is offline - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = expect_migration_error(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
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
    assert!(
        !mssql::is_database_online(&connection_string, &database_name).await,
        "expected the database to still be offline after the run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    mssql::set_whether_the_database_is_online(&connection_string, &database_name, true).await;
    assert!(
        !mssql::is_table_existing(&connection_string, &database_name, "DbMigrationsRun").await,
        "expected the tracking table to not exist"
    );
    let tables = mssql::read_the_user_table_names(&connection_string, &database_name).await;
    assert!(
        tables.is_empty(),
        "expected no tables at all — no tracking table and no script ran, got {tables:?}"
    );
    eprintln!("[4/{TOTAL}] END checking the tables - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_no_mssql_database_when_delete_runs_then_nothing_changes() {
    // What it asserts.
    // A new SQL Server container gets a database name that no database has. The
    // database does not exist before the delete. The delete succeeds, logs
    // "DeleteDatabaseIfExistAsync has executed" at INFO, and logs no
    // "DeleteDatabaseIfExistAsync executed with error". The database still does
    // not exist afterwards.
    const TOTAL: u8 = 3;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected deleting a database that does not exist to be a no-op",
    );
    assert_logged!("INFO", "DeleteDatabaseIfExistAsync has executed");
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync executed with error"),
        "expected no error to be logged when there was nothing to delete"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN checking the database");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to still not exist afterwards"
    );
    eprintln!("[3/{TOTAL}] END checking the database - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_mssql_database_with_data_when_migrations_run_then_its_data_is_kept() {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name, created up
    // front with a marker_table in it, and the mssql fixture scripts. Before the
    // run, the database and marker_table exist, and DbMigrationsRun does not.
    // The migrations run and succeed, log "setup database executed successfully" and
    // "migration process executed successfully" at INFO, and log no "setup
    // database executed with errors". The marker_table still exists, and the
    // tracking table holds three rows: 20211230_001_CreateDB.sql,
    // 20211230_002_Script2.sql and 20211231_001_Script1.sql, in that order.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = determine_a_unique_database_name("testmssql");
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating the database with a marker table");
    mssql::create_the_database(&connection_string, &database_name).await;
    mssql::execute_in_the_database(
        &connection_string,
        &database_name,
        "CREATE TABLE marker_table (Id int IDENTITY(1,1) PRIMARY KEY)",
    )
    .await;
    assert!(
        mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to exist before the migration run"
    );
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "marker_table").await,
        "expected the marker table to exist before the run"
    );
    assert!(
        !mssql::is_table_existing(&connection_string, &database_name, "DbMigrationsRun").await,
        "expected no tracking table before the run"
    );
    eprintln!("[2/{TOTAL}] END creating the database with a marker table - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed against an already existing database",
    );
    assert_logged!("INFO", "setup database executed successfully");
    assert_logged!("INFO", "migration process executed successfully");
    assert!(
        !logs_contain("setup database executed with errors"),
        "expected no error for a database that was already there"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the marker table and the tracking table");
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "marker_table").await,
        "expected the pre-existing table to survive — the database must not be recreated"
    );
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        [
            "20211230_001_CreateDB.sql",
            "20211230_002_Script2.sql",
            "20211231_001_Script1.sql",
        ],
        "expected all three fixture scripts to be tracked in order, got {rows:#?}"
    );
    eprintln!("[4/{TOTAL}] END checking the marker table and the tracking table - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_mssql_database_name_with_a_quote_and_a_bracket_when_migrations_run_then_every_script_is_applied()
 {
    // What it asserts.
    // A new SQL Server container gets a database name that ends in
    // ]o'brien, and the mssql fixture scripts with
    // 20211230_001_CreateDB.sql excluded. The database does not exist
    // before the delete. The delete succeeds, and the database still does
    // not exist after it. The migrations run and succeed, and log
    // "migration process executed successfully" at INFO. The database then
    // exists, and the tracking table holds 20211230_002_Script2.sql and
    // 20211231_001_Script1.sql, in that order. A second delete succeeds,
    // and the database no longer exists.
    const TOTAL: u8 = 5;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let database_name = format!("{}]o'brien", determine_a_unique_database_name("testmssql"));
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the delete"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts"),
            ["20211230_001_CreateDB.sql".to_string()],
        ),
        "invalid migration configuration",
    );
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected the delete to succeed for a name with a quote and a bracket",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    expect_ok(
        try_apply_migrations(
            DatabaseKind::Mssql,
            &migration_configuration,
            &AtomicBool::new(false),
        )
        .await,
        "expected the migration run to succeed for a name with a quote and a bracket",
    );
    assert_logged!("INFO", "migration process executed successfully");
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and the tracking table");
    assert!(
        mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the run to create the database"
    );
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20211230_002_Script2.sql", "20211231_001_Script1.sql"],
        "expected both scripts to be tracked in order, got {rows:#?}"
    );
    eprintln!("[4/{TOTAL}] END checking the database and the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN deleting the database again");
    expect_ok(
        try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await,
        "expected the second delete to succeed",
    );
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to be gone after the second delete"
    );
    eprintln!("[5/{TOTAL}] END deleting the database again - passed");
}

#[cfg(feature = "mssql")]
#[tokio::test]
#[traced_test]
async fn test_given_slow_first_mssql_script_when_the_run_is_cancelled_during_it_then_later_scripts_do_not_run()
 {
    // What it asserts.
    // A new SQL Server container gets a database with a unique name and the
    // cancel fixtures: a slow first script and a later script. The database
    // does not exist before the run. The migrations run, and the cancel flag
    // is set while SQL Server runs the WAITFOR of the first script. The run
    // returns success, logs "migration process was canceled" at WARN, and
    // logs no "canceled from the outside". The tracking table holds exactly
    // 20220201_001_SlowScript.sql. slow_table exists, and later_table does
    // not.
    const TOTAL: u8 = 4;

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations and cancelling during the first script");
    let database_name = determine_a_unique_database_name("testmssql");
    assert!(
        !mssql::is_database_existing(&connection_string, &database_name).await,
        "expected the database to not exist before the run"
    );
    let migration_configuration = expect_ok(
        MigrationConfiguration::new(
            connection_string.as_str(),
            database_name.as_str(),
            mssql::determine_the_fixtures_path("test_scripts_cancel"),
            Vec::<String>::new(),
        ),
        "invalid migration configuration",
    );
    let is_cancelled = AtomicBool::new(false);
    let run = try_apply_migrations(DatabaseKind::Mssql, &migration_configuration, &is_cancelled);
    let cancel = async {
        let mut tries = 0_u32;
        while !mssql::is_query_running(&connection_string, "WAITFOR").await {
            tries += 1;
            assert!(tries < 200, "expected the slow script to start within 10 s");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        is_cancelled.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(run, cancel);
    expect_ok(
        result,
        "expected a run cancelled between scripts to report success",
    );
    assert_logged!("WARN", "migration process was canceled");
    assert!(
        !logs_contain("canceled from the outside"),
        "expected the cancel between scripts, not before the start"
    );
    eprintln!(
        "[2/{TOTAL}] END running the migrations and cancelling during the first script - passed"
    );

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = mssql::read_the_tracking_rows(&connection_string, &database_name).await;
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.filename.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220201_001_SlowScript.sql"],
        "expected only the slow script to be tracked, got {rows:#?}"
    );
    eprintln!("[3/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    assert!(
        mssql::is_table_existing(&connection_string, &database_name, "slow_table").await,
        "expected slow_table to have been created by the slow script"
    );
    assert!(
        !mssql::is_table_existing(&connection_string, &database_name, "later_table").await,
        "expected later_table to not exist, because the run stopped before the later script"
    );
    eprintln!("[4/{TOTAL}] END checking the tables - passed");
}
