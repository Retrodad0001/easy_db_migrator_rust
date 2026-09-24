#![allow(clippy::panic, missing_docs)]

mod test_support;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use easy_db_migrator_rust::{
    DatabaseKind, MigrationConfiguration, try_apply_migrations, try_delete_database_if_exists,
};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, Row};
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;
use tracing_test::traced_test;

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
    const EXCLUDED_SCRIPT: &str = "20211230_001_DoStuffScript.sql";
    const FIRST_SCRIPT: &str = "20211230_002_Script2p.sql";
    const SECOND_SCRIPT: &str = "20211231_001_Script1p.sql";
    const INFO: &str = " INFO ";

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.subsec_nanos(),
        Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
    };
    let database_name = format!("testpostgres{}{nanos}", std::process::id());
    let fixtures_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/postgres")
        .join("test_scripts");
    let configuration = MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        fixtures_path,
        [EXCLUDED_SCRIPT.to_string()],
    );
    let migration_configuration = match configuration {
        Ok(migration_configuration) => migration_configuration,
        Err(error) => {
            panic!("the migration configuration should be valid, and it is not: {error:?}")
        }
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let options = match PgConnectOptions::from_str(&connection_string) {
        Ok(options) => options.database("postgres"),
        Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
    };
    let mut connection = match PgConnection::connect_with(&options).await {
        Ok(connection) => connection,
        Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
    };
    let found: Option<i32> =
        match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
            .bind(&database_name)
            .fetch_optional(&mut connection)
            .await
        {
            Ok(found) => found,
            Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
        };
    assert!(
        found.is_none(),
        "the database should not exist before the delete, and it does"
    );
    let deleted =
        try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await;
    assert!(
        deleted.is_ok(),
        "the delete should succeed, and it reports {deleted:?}"
    );
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "DeleteDatabaseIfExistAsync has executed";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(INFO)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    let found: Option<i32> =
        match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
            .bind(&database_name)
            .fetch_optional(&mut connection)
            .await
        {
            Ok(found) => found,
            Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
        };
    assert!(
        found.is_none(),
        "the database should not exist after the delete, and it does"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    let applied = try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await;
    let after = Utc::now();
    assert!(
        applied.is_ok(),
        "the migration run should succeed, and it reports {applied:?}"
    );
    logs_assert(|lines: &[&str]| {
        const EXPECTED: [&str; 5] = [
            "setup database executed successfully",
            "script was run",
            FIRST_SCRIPT,
            SECOND_SCRIPT,
            "migration process executed successfully",
        ];
        for expected in EXPECTED {
            let matching: Vec<&&str> = lines
                .iter()
                .filter(|line| line.contains(expected))
                .collect();
            if !matching.iter().any(|line| line.contains(INFO)) {
                return Err(format!(
                    "the log should hold {expected:?} at INFO, and it holds {matching:?}"
                ));
            }
        }
        Ok(())
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let options = match PgConnectOptions::from_str(&connection_string) {
        Ok(options) => options.database(&database_name),
        Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
    };
    let mut migrated = match PgConnection::connect_with(&options).await {
        Ok(migrated) => migrated,
        Err(error) => {
            panic!("the migrated database should accept a connection, and it does not: {error:?}")
        }
    };
    let rows =
        match sqlx::query("SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id")
            .fetch_all(&mut migrated)
            .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
    let [first, second] = rows.as_slice() else {
        panic!(
            "the tracking table should hold 2 rows, and it holds {}",
            rows.len()
        );
    };
    let first_filename: String = first.get("filename");
    assert_eq!(
        first_filename, FIRST_SCRIPT,
        "the first tracking row should name {FIRST_SCRIPT}, and it names {first_filename}"
    );
    let first_executed_at: DateTime<Utc> = first.get("executed_at");
    assert!(
        before <= first_executed_at && first_executed_at <= after,
        "the first row should be stamped between {before} and {after}, and it is stamped {first_executed_at}"
    );
    let first_version: String = first.get("version");
    assert_eq!(
        first_version,
        env!("CARGO_PKG_VERSION"),
        "the first row should carry the crate version, and it carries {first_version}"
    );
    let second_filename: String = second.get("filename");
    assert_eq!(
        second_filename, SECOND_SCRIPT,
        "the second tracking row should name {SECOND_SCRIPT}, and it names {second_filename}"
    );
    let second_executed_at: DateTime<Utc> = second.get("executed_at");
    assert!(
        before <= second_executed_at && second_executed_at <= after,
        "the second row should be stamped between {before} and {after}, and it is stamped {second_executed_at}"
    );
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    const CUSTOMERS: &str = "customers";
    let customers: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(CUSTOMERS)
        .fetch_one(&mut migrated)
        .await
    {
        Ok(customers) => customers,
        Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
    };
    assert!(
        customers.is_some(),
        "{CUSTOMERS} should have been created by {FIRST_SCRIPT}, and it does not exist"
    );
    const DISTRIBUTORS: &str = "distributors";
    let distributors: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(DISTRIBUTORS)
        .fetch_one(&mut migrated)
        .await
    {
        Ok(distributors) => distributors,
        Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
    };
    assert!(
        distributors.is_some(),
        "{DISTRIBUTORS} should have been created by {SECOND_SCRIPT}, and it does not exist"
    );
    const SCHOOLS: &str = "schools";
    let schools: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(SCHOOLS)
        .fetch_one(&mut migrated)
        .await
    {
        Ok(schools) => schools,
        Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
    };
    assert!(
        schools.is_none(),
        "{SCHOOLS} should not exist because {EXCLUDED_SCRIPT} was excluded, and it exists"
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
    const FIRST_SCRIPT: &str = "20211230_002_Script2p.sql";
    const SECOND_SCRIPT: &str = "20211231_001_Script1p.sql";

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        ["20211230_001_DoStuffScript.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_2 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_2,
        "expected the database to not exist after the delete, before the first run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations the first time");
    let first_run_started_at = Utc::now();
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the first migration run to succeed: {error:?}"),
    };
    let first_run_finished_at = Utc::now();
    let rows_after_the_first_run = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let filenames_after_the_first_run: Vec<&str> = rows_after_the_first_run
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames_after_the_first_run,
        ["20211230_002_Script2p.sql", "20211231_001_Script1p.sql"],
        "expected the first run to track both scripts, before the second run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations the first time - passed");

    eprintln!("[4/{TOTAL}] BEGIN running the migrations the second time");
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the second migration run to succeed: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup versioning table executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was not run because script was already executed";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[4/{TOTAL}] END running the migrations the second time - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(
        first.0, FIRST_SCRIPT,
        "the first tracking row should name {}, and it names {}",
        "20211230_002_Script2p.sql", first.0
    );
    assert!(
        first_run_started_at <= first.1,
        "the first tracking row should be stamped at or after {}, and it is stamped {}",
        first_run_started_at,
        first.1
    );
    assert!(
        first.1 <= first_run_finished_at,
        "the first tracking row should be stamped at or before {}, and it is stamped {}",
        first_run_finished_at,
        first.1
    );
    assert_eq!(
        second.0, SECOND_SCRIPT,
        "the second tracking row should name {}, and it names {}",
        "20211231_001_Script1p.sql", second.0
    );
    assert!(
        first_run_started_at <= second.1,
        "the second tracking row should be stamped at or after {}, and it is stamped {}",
        first_run_started_at,
        second.1
    );
    assert!(
        second.1 <= first_run_finished_at,
        "the second tracking row should be stamped at or before {}, and it is stamped {}",
        first_run_finished_at,
        second.1
    );
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[6/{TOTAL}] BEGIN checking the tables");
    let table_existing = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("customers")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing,
        "expected customers table to have been created by 20211230_002_Script2p.sql"
    );
    let table_existing_2 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("distributors")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing_2,
        "expected distributors table to have been created by 20211231_001_Script1p.sql"
    );
    let table_existing_3 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("schools")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        !table_existing_3,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        ["20211230_001_DoStuffScript.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database and setting the cancel flag");
    let existing_3 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_3,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_4 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_4,
        "expected the database to not exist after the delete, before the run"
    );
    let is_cancelled = AtomicBool::new(false);
    is_cancelled.store(true, Ordering::SeqCst);
    eprintln!("[2/{TOTAL}] END deleting the database and setting the cancel flag - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &is_cancelled,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected a cancelled run to still report success: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process was canceled from the outside";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database");
    let existing_5 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_5,
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
    const GOOD_SCRIPT: &str = "20220101_001_GoodScriptp.sql";
    const RUN_FAILURE: &str = "migration process executed with errors";

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts_failure"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_6 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_6,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_7 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_7,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    let message = match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!("expected the migration run to report failure when a script fails"),
        Err(error_kind) => error_kind.to_string(),
    };
    let after = Utc::now();
    assert_eq!(
        message, RUN_FAILURE,
        "the run should report {}, and it reports {}",
        "migration process executed with errors", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was not completed due to exception";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was skipped due to exception in previous script";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let [only] = rows.as_slice() else {
        panic!("expected exactly 1 tracking row for the one script that succeeded, got {rows:#?}");
    };
    assert_eq!(
        only.0, GOOD_SCRIPT,
        "the only tracking row should name {}, and it names {}",
        "20220101_001_GoodScriptp.sql", only.0
    );
    assert!(
        before <= only.1 && only.1 <= after,
        "the only tracking row should be stamped between {} and {}, and it is stamped {}",
        before,
        after,
        only.1
    );
    assert_eq!(
        only.2,
        env!("CARGO_PKG_VERSION"),
        "the only tracking row should carry the crate version, and it carries {}",
        only.2
    );
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    let tables = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("pg_tables should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| row.get("tablename"))
            .collect::<Vec<String>>()
    };
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("does_not_exist"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_8 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_8,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_9 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_9,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => {
            panic!("expected the migration run to report failure when the scripts cannot be loaded")
        }
        Err(error_kind) => error_kind.to_string(),
    };
    const LOAD_FAILURE: &str =
        "One or more scripts could not be loaded, is the sequence patterns correct?";
    assert_eq!(
        message, LOAD_FAILURE,
        "the run should report {}, and it reports {}",
        LOAD_FAILURE, message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed with errors";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the scripts could not be loaded"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and its tables");
    let existing_10 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        existing_10,
        "expected the database to exist, since it is created before scripts are loaded"
    );
    let tables = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("pg_tables should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| row.get("tablename"))
            .collect::<Vec<String>>()
    };
    assert_eq!(
        tables,
        vec!["dbmigrationsrun".to_string()],
        "expected only the tracking table — no migration script may have created anything"
    );
    eprintln!("[4/{TOTAL}] END checking the database and its tables - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
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
    const VERSIONING_FAILURE: &str = "setup versioning table executed with errors";

    eprintln!("[1/{TOTAL}] BEGIN starting the postgres container");
    let (_container, connection_string) = postgres::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating a database that refuses connections");
    postgres::create_the_database(&connection_string, &database_name).await;
    postgres::set_whether_the_database_accepts_connections(
        &connection_string,
        &database_name,
        false,
    )
    .await;
    let existing_11 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(existing_11, "expected the database to exist before the run");
    let accepting_connections = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let accepting: Option<bool> =
            match sqlx::query_scalar("SELECT datallowconn FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(accepting) => accepting,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        accepting.unwrap_or(false)
    };
    assert!(
        !accepting_connections,
        "expected the database to refuse connections before the run"
    );
    eprintln!("[2/{TOTAL}] END creating a database that refuses connections - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!(
            "expected the migration run to report failure when the tracking table cannot be created"
        ),
        Err(error_kind) => error_kind.to_string(),
    };
    assert_eq!(
        message, VERSIONING_FAILURE,
        "the run should report {}, and it reports {}",
        "setup versioning table executed with errors", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed with errors";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("setup versioning table executed successfully"),
        "expected the versioning-table step to not report success"
    );
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the tracking table could not be created"
    );
    let accepting_connections_2 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let accepting: Option<bool> =
            match sqlx::query_scalar("SELECT datallowconn FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(accepting) => accepting,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        accepting.unwrap_or(false)
    };
    assert!(
        !accepting_connections_2,
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
    let table_existing_4 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("dbmigrationsrun")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        !table_existing_4,
        "expected the tracking table to not exist"
    );
    let tables = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("pg_tables should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| row.get("tablename"))
            .collect::<Vec<String>>()
    };
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let existing_12 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_12,
        "expected the database to not exist before the delete"
    );
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => {
            panic!("expected deleting a database that does not exist to be a no-op: {error:?}")
        }
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "DeleteDatabaseIfExistAsync has executed";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync executed with error"),
        "expected no error to be logged when there was nothing to delete"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN checking the database");
    let existing_13 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_13,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the postgres container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating the database with a marker table");
    postgres::create_the_database(&connection_string, &database_name).await;
    postgres::execute_in_the_database(
        &connection_string,
        &database_name,
        "CREATE TABLE marker_table (id integer PRIMARY KEY)",
    )
    .await;
    let existing_14 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        existing_14,
        "expected the database to exist before the migration run"
    );
    let table_existing_5 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("marker_table")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing_5,
        "expected the marker table to exist before the run"
    );
    let table_existing_6 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("dbmigrationsrun")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        !table_existing_6,
        "expected no tracking table before the run"
    );
    eprintln!("[2/{TOTAL}] END creating the database with a marker table - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the migration run to succeed against an already existing database: {error:?}"
        ),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("setup database executed with errors"),
        "expected no error for a database that was already there"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the marker table and the tracking table");
    let table_existing_7 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("marker_table")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing_7,
        "expected the pre-existing table to survive — the database must not be recreated"
    );
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let existing_15 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_15,
        "expected the database to not exist before the delete"
    );
    let migration_configuration = match MigrationConfiguration::new(
        format!("{connection_string}/postgres?sslmode=disable"),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        ["20211230_001_DoStuffScript.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the delete to succeed with a database and a query in the connection string: {error:?}"
        ),
    };
    let existing_16 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_16,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the migration run to succeed with a database and a query in the connection string: {error:?}"
        ),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
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
    const DELETE_FAILURE: &str = "DeleteDatabaseIfExistAsync executed with error";

    eprintln!("[1/{TOTAL}] BEGIN checking that no server answers on 127.0.0.1:1");
    assert!(
        std::net::TcpStream::connect("127.0.0.1:1").is_err(),
        "expected no server to answer on 127.0.0.1:1"
    );
    eprintln!("[1/{TOTAL}] END checking that no server answers on 127.0.0.1:1 - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let migration_configuration = match MigrationConfiguration::new(
        "postgres://postgres:postgres@127.0.0.1:1",
        "testpostgres",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let message =
        match try_delete_database_if_exists(DatabaseKind::Postgresql, &migration_configuration)
            .await
        {
            Ok(()) => panic!("expected the delete to fail when no server answers"),
            Err(error_kind) => error_kind.to_string(),
        };
    assert_eq!(
        message, DELETE_FAILURE,
        "the run should report {}, and it reports {}",
        "DeleteDatabaseIfExistAsync executed with error", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
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
    const SETUP_FAILURE: &str = "setup database executed with errors";

    eprintln!("[1/{TOTAL}] BEGIN checking that no server answers on 127.0.0.1:1");
    assert!(
        std::net::TcpStream::connect("127.0.0.1:1").is_err(),
        "expected no server to answer on 127.0.0.1:1"
    );
    eprintln!("[1/{TOTAL}] END checking that no server answers on 127.0.0.1:1 - passed");

    eprintln!("[2/{TOTAL}] BEGIN running the migrations");
    let migration_configuration = match MigrationConfiguration::new(
        "postgres://postgres:postgres@127.0.0.1:1",
        "testpostgres",
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let message = match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!("expected the run to fail when no server answers"),
        Err(error_kind) => error_kind.to_string(),
    };
    assert_eq!(
        message, SETUP_FAILURE,
        "the run should report {}, and it reports {}",
        "setup database executed with errors", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed with errors";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let existing_17 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_17,
        "expected the database to not exist before the run"
    );
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts_with_folder"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the run to succeed and skip the folder: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[2/{TOTAL}] END running the migrations - passed");

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table and the tables");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220401_001_FolderScriptp.sql"],
        "expected only the script to be tracked, not the folder, got {rows:#?}"
    );
    let table_existing_8 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("folder_table")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing_8,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let existing_18 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_18,
        "expected the database to not exist before the run"
    );
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts_cancel"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let is_cancelled = AtomicBool::new(false);
    let run = try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &is_cancelled,
    );
    let cancel = async {
        let mut tries = 0_u32;
        while !{
            let options = match PgConnectOptions::from_str(&connection_string) {
                Ok(options) => options.database("postgres"),
                Err(error) => {
                    panic!("the connection string should be valid, and it is not: {error:?}")
                }
            };
            let mut connection = match PgConnection::connect_with(&options).await {
                Ok(connection) => connection,
                Err(error) => {
                    panic!("postgres should accept a connection, and it does not: {error:?}")
                }
            };
            let running: i64 = match sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE query LIKE $1 AND pid <> pg_backend_pid()",
        )
        .bind(format!("%{}%", "pg_sleep"))
        .fetch_one(&mut connection)
        .await
        {
            Ok(running) => running,
            Err(error) => panic!("pg_stat_activity should be readable, and it is not: {error:?}"),
        };
            running > 0
        } {
            tries += 1;
            assert!(tries < 200, "expected the slow script to start within 10 s");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        is_cancelled.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(run, cancel);
    match result {
        Ok(value) => value,
        Err(error) => {
            panic!("expected a run cancelled between scripts to report success: {error:?}")
        }
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process was canceled";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("canceled from the outside"),
        "expected the cancel between scripts, not before the start"
    );
    eprintln!(
        "[2/{TOTAL}] END running the migrations and cancelling during the first script - passed"
    );

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220201_001_SlowScriptp.sql"],
        "expected only the slow script to be tracked, got {rows:#?}"
    );
    eprintln!("[3/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    let table_existing_9 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("slow_table")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        table_existing_9,
        "expected slow_table to have been created by the slow script"
    );
    let table_existing_10 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let regclass: Option<String> = match sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind("later_table")
            .fetch_one(&mut connection)
            .await
        {
            Ok(regclass) => regclass,
            Err(error) => panic!("to_regclass should answer, and it does not: {error:?}"),
        };
        regclass.is_some()
    };
    assert!(
        !table_existing_10,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testpostgres{}{nanos}", std::process::id())
    };
    let existing_19 = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database("postgres"),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let found: Option<i32> =
            match sqlx::query_scalar("SELECT 1 FROM pg_database WHERE datname = $1")
                .bind(&database_name)
                .fetch_optional(&mut connection)
                .await
            {
                Ok(found) => found,
                Err(error) => panic!("pg_database should be readable, and it is not: {error:?}"),
            };
        found.is_some()
    };
    assert!(
        !existing_19,
        "expected the database to not exist before the run"
    );
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/postgres")
            .join("test_scripts_not_utf8"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let message = match try_apply_migrations(
        DatabaseKind::Postgresql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!("expected the run to fail when a script is not UTF-8"),
        Err(error_kind) => error_kind.to_string(),
    };
    const LOAD_FAILURE: &str =
        "One or more scripts could not be loaded, is the sequence patterns correct?";
    assert_eq!(
        message, LOAD_FAILURE,
        "the run should report {}, and it reports {}",
        LOAD_FAILURE, message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when a script could not be read"
    );
    eprintln!("[2/{TOTAL}] END running the migrations - passed");

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let options = match PgConnectOptions::from_str(&connection_string) {
            Ok(options) => options.database(&database_name),
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let mut connection = match PgConnection::connect_with(&options).await {
            Ok(connection) => connection,
            Err(error) => panic!("postgres should accept a connection, and it does not: {error:?}"),
        };
        let rows = match sqlx::query(
            "SELECT filename, executed_at, version FROM DbMigrationsRun ORDER BY id",
        )
        .fetch_all(&mut connection)
        .await
        {
            Ok(rows) => rows,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("filename"),
                    row.get::<DateTime<Utc>, _>("executed_at"),
                    row.get::<String, _>("version"),
                )
            })
            .collect::<Vec<(String, DateTime<Utc>, String)>>()
    };
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
    // successfully", "script was run", FIRST_SCRIPT,
    // "20211231_001_Script1.sql" and "migration process executed successfully" at
    // INFO. The tracking table holds exactly two rows: 20211230_002_Script2.sql
    // first and 20211231_001_Script1.sql second. Each row has an executed_at
    // between the time before and the time after the run, and the first row has
    // the crate version. The tables bb and aa exist, and placeholder does not.
    const TOTAL: u8 = 5;
    const FIRST_SCRIPT: &str = "20211230_002_Script2.sql";
    const SECOND_SCRIPT: &str = "20211231_001_Script1.sql";

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        ["20211230_001_CreateDB.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_20 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_20,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "DeleteDatabaseIfExistAsync has executed";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    let existing_21 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_21,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the migration run to succeed: {error:?}"),
    };
    let after = Utc::now();
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was run";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "20211230_002_Script2.sql";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "20211231_001_Script1.sql";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(
        first.0, FIRST_SCRIPT,
        "the first tracking row should name {}, and it names {}",
        "20211230_002_Script2.sql", first.0
    );
    assert!(
        before <= first.1 && first.1 <= after,
        "the first tracking row should be stamped between {} and {}, and it is stamped {}",
        before,
        after,
        first.1
    );
    assert_eq!(
        first.2,
        env!("CARGO_PKG_VERSION"),
        "the first tracking row should carry the crate version, and it carries {}",
        first.2
    );
    assert_eq!(
        second.0, SECOND_SCRIPT,
        "the second tracking row should name {}, and it names {}",
        "20211231_001_Script1.sql", second.0
    );
    assert!(
        before <= second.1 && second.1 <= after,
        "the second tracking row should be stamped between {} and {}, and it is stamped {}",
        before,
        after,
        second.1
    );
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    let table_existing_11 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("bb");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_11,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    let table_existing_12 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("aa");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_12,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    let table_existing_13 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("placeholder");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !table_existing_13,
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
    const FIRST_SCRIPT: &str = "20211230_002_Script2.sql";
    const SECOND_SCRIPT: &str = "20211231_001_Script1.sql";

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        ["20211230_001_CreateDB.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_22 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_22,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_23 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_23,
        "expected the database to not exist after the delete, before the first run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations the first time");
    let first_run_started_at = Utc::now();
    match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the first migration run to succeed: {error:?}"),
    };
    let first_run_finished_at = Utc::now();
    let rows_after_the_first_run = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let filenames_after_the_first_run: Vec<&str> = rows_after_the_first_run
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames_after_the_first_run,
        ["20211230_002_Script2.sql", "20211231_001_Script1.sql"],
        "expected the first run to track both scripts, before the second run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations the first time - passed");

    eprintln!("[4/{TOTAL}] BEGIN running the migrations the second time");
    match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!("expected the second migration run to succeed: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup versioning table executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was not run because script was already executed";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[4/{TOTAL}] END running the migrations the second time - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let [first, second] = rows.as_slice() else {
        panic!("expected exactly 2 tracking rows, got {rows:#?}");
    };
    assert_eq!(
        first.0, FIRST_SCRIPT,
        "the first tracking row should name {}, and it names {}",
        "20211230_002_Script2.sql", first.0
    );
    assert!(
        first_run_started_at <= first.1,
        "the first tracking row should be stamped at or after {}, and it is stamped {}",
        first_run_started_at,
        first.1
    );
    assert!(
        first.1 <= first_run_finished_at,
        "the first tracking row should be stamped at or before {}, and it is stamped {}",
        first_run_finished_at,
        first.1
    );
    assert_eq!(
        second.0, SECOND_SCRIPT,
        "the second tracking row should name {}, and it names {}",
        "20211231_001_Script1.sql", second.0
    );
    assert!(
        first_run_started_at <= second.1,
        "the second tracking row should be stamped at or after {}, and it is stamped {}",
        first_run_started_at,
        second.1
    );
    assert!(
        second.1 <= first_run_finished_at,
        "the second tracking row should be stamped at or before {}, and it is stamped {}",
        first_run_finished_at,
        second.1
    );
    eprintln!("[5/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[6/{TOTAL}] BEGIN checking the tables");
    let table_existing_14 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("bb");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_14,
        "expected table bb to have been created by 20211230_002_Script2.sql"
    );
    let table_existing_15 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("aa");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_15,
        "expected table aa to have been created by 20211231_001_Script1.sql"
    );
    let table_existing_16 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("placeholder");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !table_existing_16,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        ["20211230_001_CreateDB.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database and setting the cancel flag");
    let existing_24 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_24,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_25 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_25,
        "expected the database to not exist after the delete, before the run"
    );
    let is_cancelled = AtomicBool::new(false);
    is_cancelled.store(true, Ordering::SeqCst);
    eprintln!("[2/{TOTAL}] END deleting the database and setting the cancel flag - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(DatabaseKind::Mssql, &migration_configuration, &is_cancelled).await {
        Ok(value) => value,
        Err(error) => panic!("expected a cancelled run to still report success: {error:?}"),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process was canceled from the outside";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database");
    let existing_26 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_26,
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
    const GOOD_SCRIPT: &str = "20220101_001_GoodScript.sql";
    const RUN_FAILURE: &str = "migration process executed with errors";

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts_failure"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_27 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_27,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_28 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_28,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let before = Utc::now();
    let message = match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!("expected the migration run to report failure when a script fails"),
        Err(error_kind) => error_kind.to_string(),
    };
    let after = Utc::now();
    assert_eq!(
        message, RUN_FAILURE,
        "the run should report {}, and it reports {}",
        "migration process executed with errors", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was not completed due to exception";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "script was skipped due to exception in previous script";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let [only] = rows.as_slice() else {
        panic!("expected exactly 1 tracking row for the one script that succeeded, got {rows:#?}");
    };
    assert_eq!(
        only.0, GOOD_SCRIPT,
        "the only tracking row should name {}, and it names {}",
        "20220101_001_GoodScript.sql", only.0
    );
    assert!(
        before <= only.1 && only.1 <= after,
        "the only tracking row should be stamped between {} and {}, and it is stamped {}",
        before,
        after,
        only.1
    );
    assert_eq!(
        only.2,
        env!("CARGO_PKG_VERSION"),
        "the only tracking row should carry the crate version, and it carries {}",
        only.2
    );
    eprintln!("[4/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN checking the tables");
    let tables = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT name FROM sys.tables ORDER BY name")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("sys.tables should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the table rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut names = Vec::with_capacity(rows.len());
        for row in rows {
            let name: &str = match row.get("name") {
                Some(name) => name,
                None => panic!("every row should carry a name, and one does not"),
            };
            names.push(name.to_string());
        }
        names
    };
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("does_not_exist"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    let existing_29 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_29,
        "expected the database to not exist before the delete"
    );
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected DeleteDatabaseIfExistAsync to succeed: {error:?}"),
    };
    let existing_30 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_30,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => {
            panic!("expected the migration run to report failure when the scripts cannot be loaded")
        }
        Err(error_kind) => error_kind.to_string(),
    };
    const LOAD_FAILURE: &str =
        "One or more scripts could not be loaded, is the sequence patterns correct?";
    assert_eq!(
        message, LOAD_FAILURE,
        "the run should report {}, and it reports {}",
        LOAD_FAILURE, message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed with errors";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the scripts could not be loaded"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and its tables");
    let existing_31 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        existing_31,
        "expected the database to exist, since it is created before scripts are loaded"
    );
    let tables = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT name FROM sys.tables ORDER BY name")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("sys.tables should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the table rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut names = Vec::with_capacity(rows.len());
        for row in rows {
            let name: &str = match row.get("name") {
                Some(name) => name,
                None => panic!("every row should carry a name, and one does not"),
            };
            names.push(name.to_string());
        }
        names
    };
    assert_eq!(
        tables,
        vec!["DbMigrationsRun".to_string()],
        "expected only the tracking table — no migration script may have created anything"
    );
    eprintln!("[4/{TOTAL}] END checking the database and its tables - passed");

    eprintln!("[5/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
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
    const VERSIONING_FAILURE: &str = "setup versioning table executed with errors";

    eprintln!("[1/{TOTAL}] BEGIN starting the mssql container");
    let (_container, connection_string) = mssql::start_the_container().await;
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating a database that is offline");
    mssql::create_the_database(&connection_string, &database_name).await;
    mssql::set_whether_the_database_is_online(&connection_string, &database_name, false).await;
    let existing_32 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(existing_32, "expected the database to exist before the run");
    let online = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT state_desc FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.and_then(|row| {
            row.get::<&str, _>("state_desc")
                .map(|state| state == "ONLINE")
        })
        .unwrap_or(false)
    };
    assert!(
        !online,
        "expected the database to be offline before the run"
    );
    eprintln!("[2/{TOTAL}] END creating a database that is offline - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    let message = match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(()) => panic!(
            "expected the migration run to report failure when the tracking table cannot be created"
        ),
        Err(error_kind) => error_kind.to_string(),
    };
    assert_eq!(
        message, VERSIONING_FAILURE,
        "the run should report {}, and it reports {}",
        "setup versioning table executed with errors", message
    );
    logs_assert(|lines: &[&str]| {
        let expected: &str = &message;
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(expected))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {expected:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed with errors";
        const LEVEL: &str = " ERROR ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at ERROR, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("setup versioning table executed successfully"),
        "expected the versioning-table step to not report success"
    );
    assert!(
        !logs_contain("script was run"),
        "expected no script to run when the tracking table could not be created"
    );
    let online_2 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT state_desc FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.and_then(|row| {
            row.get::<&str, _>("state_desc")
                .map(|state| state == "ONLINE")
        })
        .unwrap_or(false)
    };
    assert!(
        !online_2,
        "expected the database to still be offline after the run"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    mssql::set_whether_the_database_is_online(&connection_string, &database_name, true).await;
    let table_existing_17 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("DbMigrationsRun");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !table_existing_17,
        "expected the tracking table to not exist"
    );
    let tables = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT name FROM sys.tables ORDER BY name")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("sys.tables should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the table rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut names = Vec::with_capacity(rows.len());
        for row in rows {
            let name: &str = match row.get("name") {
                Some(name) => name,
                None => panic!("every row should carry a name, and one does not"),
            };
            names.push(name.to_string());
        }
        names
    };
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let existing_33 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_33,
        "expected the database to not exist before the delete"
    );
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN deleting the database");
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => {
            panic!("expected deleting a database that does not exist to be a no-op: {error:?}")
        }
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "DeleteDatabaseIfExistAsync has executed";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("DeleteDatabaseIfExistAsync executed with error"),
        "expected no error to be logged when there was nothing to delete"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN checking the database");
    let existing_34 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_34,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    eprintln!("[1/{TOTAL}] END starting the mssql container - passed");

    eprintln!("[2/{TOTAL}] BEGIN creating the database with a marker table");
    mssql::create_the_database(&connection_string, &database_name).await;
    mssql::execute_in_the_database(
        &connection_string,
        &database_name,
        "CREATE TABLE marker_table (Id int IDENTITY(1,1) PRIMARY KEY)",
    )
    .await;
    let existing_35 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        existing_35,
        "expected the database to exist before the migration run"
    );
    let table_existing_18 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("marker_table");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_18,
        "expected the marker table to exist before the run"
    );
    let table_existing_19 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("DbMigrationsRun");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !table_existing_19,
        "expected no tracking table before the run"
    );
    eprintln!("[2/{TOTAL}] END creating the database with a marker table - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the migration run to succeed against an already existing database: {error:?}"
        ),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "setup database executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("setup database executed with errors"),
        "expected no error for a database that was already there"
    );
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the marker table and the tracking table");
    let table_existing_20 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("marker_table");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_20,
        "expected the pre-existing table to survive — the database must not be recreated"
    );
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
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
    let database_name = format!("{}]o'brien", {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    });
    let existing_36 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_36,
        "expected the database to not exist before the delete"
    );
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts"),
        ["20211230_001_CreateDB.sql".to_string()],
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the delete to succeed for a name with a quote and a bracket: {error:?}"
        ),
    };
    let existing_37 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_37,
        "expected the database to not exist after the delete, before the run"
    );
    eprintln!("[2/{TOTAL}] END deleting the database - passed");

    eprintln!("[3/{TOTAL}] BEGIN running the migrations");
    match try_apply_migrations(
        DatabaseKind::Mssql,
        &migration_configuration,
        &AtomicBool::new(false),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => panic!(
            "expected the migration run to succeed for a name with a quote and a bracket: {error:?}"
        ),
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process executed successfully";
        const LEVEL: &str = " INFO ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at INFO, and it holds {matching:?}"
            ))
        }
    });
    eprintln!("[3/{TOTAL}] END running the migrations - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the database and the tracking table");
    let existing_38 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(existing_38, "expected the run to create the database");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20211230_002_Script2.sql", "20211231_001_Script1.sql"],
        "expected both scripts to be tracked in order, got {rows:#?}"
    );
    eprintln!("[4/{TOTAL}] END checking the database and the tracking table - passed");

    eprintln!("[5/{TOTAL}] BEGIN deleting the database again");
    match try_delete_database_if_exists(DatabaseKind::Mssql, &migration_configuration).await {
        Ok(value) => value,
        Err(error) => panic!("expected the second delete to succeed: {error:?}"),
    };
    let existing_39 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_39,
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
    let database_name = {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.subsec_nanos(),
            Err(error) => panic!("the clock should be after the epoch, and it is not: {error:?}"),
        };
        format!("testmssql{}{nanos}", std::process::id())
    };
    let existing_40 = {
        let config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
        query.bind(&database_name);
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sys.databases should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !existing_40,
        "expected the database to not exist before the run"
    );
    let migration_configuration = match MigrationConfiguration::new(
        connection_string.as_str(),
        database_name.as_str(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mssql")
            .join("test_scripts_cancel"),
        Vec::<String>::new(),
    ) {
        Ok(value) => value,
        Err(error) => panic!("invalid migration configuration: {error:?}"),
    };
    let is_cancelled = AtomicBool::new(false);
    let run = try_apply_migrations(DatabaseKind::Mssql, &migration_configuration, &is_cancelled);
    let cancel = async {
        let mut tries = 0_u32;
        while !{
            let config = match Config::from_ado_string(&connection_string) {
                Ok(config) => config,
                Err(error) => {
                    panic!("the connection string should be valid, and it is not: {error:?}")
                }
            };
            let tcp = match TcpStream::connect(config.get_addr()).await {
                Ok(tcp) => tcp,
                Err(error) => {
                    panic!("mssql should accept a connection, and it does not: {error:?}")
                }
            };
            let mut client = match Client::connect(config, tcp.compat_write()).await {
                Ok(client) => client,
                Err(error) => {
                    panic!("mssql should start a tds session, and it does not: {error:?}")
                }
            };
            let mut query = Query::new(
                "SELECT COUNT(*) FROM sys.dm_exec_requests AS r \
             CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) AS t \
             WHERE t.text LIKE @P1 AND r.session_id <> @@SPID",
            );
            query.bind(format!("%{}%", "WAITFOR"));
            let stream = match query.query(&mut client).await {
                Ok(stream) => stream,
                Err(error) => {
                    panic!("the running requests should be readable, and they are not: {error:?}")
                }
            };
            let row = match stream.into_row().await {
                Ok(row) => row,
                Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
            };
            row.and_then(|row| row.get::<i32, _>(0)).unwrap_or(0) > 0
        } {
            tries += 1;
            assert!(tries < 200, "expected the slow script to start within 10 s");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        is_cancelled.store(true, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(run, cancel);
    match result {
        Ok(value) => value,
        Err(error) => {
            panic!("expected a run cancelled between scripts to report success: {error:?}")
        }
    };
    logs_assert(|lines: &[&str]| {
        const EXPECTED: &str = "migration process was canceled";
        const LEVEL: &str = " WARN ";
        let matching: Vec<&&str> = lines
            .iter()
            .filter(|line| line.contains(EXPECTED))
            .collect();
        if matching.iter().any(|line| line.contains(LEVEL)) {
            Ok(())
        } else {
            Err(format!(
                "the log should hold {EXPECTED:?} at WARN, and it holds {matching:?}"
            ))
        }
    });
    assert!(
        !logs_contain("canceled from the outside"),
        "expected the cancel between scripts, not before the start"
    );
    eprintln!(
        "[2/{TOTAL}] END running the migrations and cancelling during the first script - passed"
    );

    eprintln!("[3/{TOTAL}] BEGIN reading the tracking table");
    let rows = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let stream = match client
            .simple_query("SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id")
            .await
        {
            Ok(stream) => stream,
            Err(error) => panic!("the tracking table should be readable, and it is not: {error:?}"),
        };
        let rows = match stream.into_first_result().await {
            Ok(rows) => rows,
            Err(error) => {
                panic!("the tracking rows should be collectable, and they are not: {error:?}")
            }
        };
        let mut tracking_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let filename: &str = match row.get("Filename") {
                Some(filename) => filename,
                None => panic!("every tracking row should carry a Filename, and one does not"),
            };
            let executed_at: DateTime<Utc> = match row.get("ExecutedAt") {
                Some(executed_at) => executed_at,
                None => panic!("every tracking row should carry an ExecutedAt, and one does not"),
            };
            let version: &str = match row.get("Version") {
                Some(version) => version,
                None => panic!("every tracking row should carry a Version, and one does not"),
            };
            tracking_rows.push((filename.to_string(), executed_at, version.to_string()));
        }
        tracking_rows
    };
    let filenames: Vec<&str> = rows
        .iter()
        .map(|tracking_row| tracking_row.0.as_str())
        .collect();
    assert_eq!(
        filenames,
        ["20220201_001_SlowScript.sql"],
        "expected only the slow script to be tracked, got {rows:#?}"
    );
    eprintln!("[3/{TOTAL}] END reading the tracking table - passed");

    eprintln!("[4/{TOTAL}] BEGIN checking the tables");
    let table_existing_21 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("slow_table");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        table_existing_21,
        "expected slow_table to have been created by the slow script"
    );
    let table_existing_22 = {
        let mut config = match Config::from_ado_string(&connection_string) {
            Ok(config) => config,
            Err(error) => panic!("the connection string should be valid, and it is not: {error:?}"),
        };
        config.database(&database_name);
        let tcp = match TcpStream::connect(config.get_addr()).await {
            Ok(tcp) => tcp,
            Err(error) => panic!("mssql should accept a connection, and it does not: {error:?}"),
        };
        let mut client = match Client::connect(config, tcp.compat_write()).await {
            Ok(client) => client,
            Err(error) => panic!("mssql should start a tds session, and it does not: {error:?}"),
        };
        let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
        query.bind("later_table");
        let stream = match query.query(&mut client).await {
            Ok(stream) => stream,
            Err(error) => panic!("sysobjects should be readable, and it is not: {error:?}"),
        };
        let row = match stream.into_row().await {
            Ok(row) => row,
            Err(error) => panic!("the row should be collectable, and it is not: {error:?}"),
        };
        row.is_some()
    };
    assert!(
        !table_existing_22,
        "expected later_table to not exist, because the run stopped before the later script"
    );
    eprintln!("[4/{TOTAL}] END checking the tables - passed");
}
