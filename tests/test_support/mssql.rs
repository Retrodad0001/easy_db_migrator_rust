use std::path::PathBuf;
use std::sync::Once;

use chrono::{DateTime, Utc};
use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{TrackingRow, expect_ok, expect_some, sweep_leftover_containers};

pub(crate) async fn start_the_container() -> (ContainerAsync<GenericImage>, String) {
    const TEST_LABEL: &str = "easy_db_migrator_rust_mssql_test";
    static SWEPT: Once = Once::new();
    SWEPT.call_once(|| sweep_leftover_containers(TEST_LABEL));

    const SA_PASSWORD: &str = "yourStrong(!)Password";
    let container = expect_ok(
        GenericImage::new("mcr.microsoft.com/mssql/server", "2022-CU26-ubuntu-22.04")
            .with_wait_for(WaitFor::message_on_stdout(
                "SQL Server is now ready for client connections",
            ))
            .with_wait_for(WaitFor::message_on_stdout("Recovery is complete"))
            .with_env_var("ACCEPT_EULA", "Y")
            .with_env_var("MSSQL_PID", "Developer")
            .with_env_var("MSSQL_SA_PASSWORD", SA_PASSWORD)
            .with_label(TEST_LABEL, "1")
            .start()
            .await,
        "failed to start mssql container",
    );

    let host = expect_ok(container.get_host().await, "failed to get host");
    let port = expect_ok(
        container.get_host_port_ipv4(1433).await,
        "failed to get port",
    );

    (
        container,
        format!(
            "Server=tcp:{host},{port};User Id=sa;Password={SA_PASSWORD};TrustServerCertificate=True;Encrypt=False;"
        ),
    )
}

#[inline]
pub(crate) fn determine_the_fixtures_path(fixture_directory: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mssql")
        .join(fixture_directory)
}

async fn connect(
    connection_string: &str,
    database_name: Option<&str>,
) -> Client<Compat<TcpStream>> {
    let mut config = expect_ok(
        Config::from_ado_string(connection_string),
        "invalid ado connection string",
    );
    if let Some(database_name) = database_name {
        config.database(database_name);
    }
    let tcp = expect_ok(
        TcpStream::connect(config.get_addr()).await,
        "failed to connect to mssql",
    );
    expect_ok(
        Client::connect(config, tcp.compat_write()).await,
        "failed to establish tds connection",
    )
}

pub(crate) async fn is_database_existing(connection_string: &str, database_name: &str) -> bool {
    let mut client = connect(connection_string, None).await;
    let mut query = Query::new("SELECT 1 FROM sys.databases WHERE name = @P1");
    query.bind(database_name);
    let row = expect_ok(
        expect_ok(
            query.query(&mut client).await,
            "failed to check database existence",
        )
        .into_row()
        .await,
        "failed to collect database-existence row",
    );
    row.is_some()
}

pub(crate) async fn is_query_running(connection_string: &str, text: &str) -> bool {
    let mut client = connect(connection_string, None).await;
    let mut query = Query::new(
        "SELECT COUNT(*) FROM sys.dm_exec_requests AS r \
         CROSS APPLY sys.dm_exec_sql_text(r.sql_handle) AS t \
         WHERE t.text LIKE @P1 AND r.session_id <> @@SPID",
    );
    query.bind(format!("%{text}%"));
    let row = expect_ok(
        expect_ok(
            query.query(&mut client).await,
            "failed to read the running requests",
        )
        .into_row()
        .await,
        "failed to collect the running-requests row",
    );
    row.and_then(|row| row.get::<i32, _>(0)).unwrap_or(0) > 0
}

pub(crate) async fn is_database_online(connection_string: &str, database_name: &str) -> bool {
    let mut client = connect(connection_string, None).await;
    let mut query = Query::new("SELECT state_desc FROM sys.databases WHERE name = @P1");
    query.bind(database_name);
    let row = expect_ok(
        expect_ok(
            query.query(&mut client).await,
            "failed to read the state of the database",
        )
        .into_row()
        .await,
        "failed to collect the database-state row",
    );
    row.and_then(|row| {
        row.get::<&str, _>("state_desc")
            .map(|state| state == "ONLINE")
    })
    .unwrap_or(false)
}

pub(crate) async fn create_the_database(connection_string: &str, database_name: &str) {
    let mut client = connect(connection_string, None).await;
    expect_ok(
        Query::new(format!("CREATE DATABASE [{database_name}]"))
            .execute(&mut client)
            .await,
        "failed to create the database up front",
    );
}

pub(crate) async fn execute_in_the_database(
    connection_string: &str,
    database_name: &str,
    sql: &str,
) {
    let mut client = connect(connection_string, Some(database_name)).await;
    expect_ok(
        Query::new(sql).execute(&mut client).await,
        "failed to run setup statement",
    );
}

pub(crate) async fn set_whether_the_database_is_online(
    connection_string: &str,
    database_name: &str,
    is_online: bool,
) {
    let mut client = connect(connection_string, None).await;
    let sql = if is_online {
        format!("ALTER DATABASE [{database_name}] SET ONLINE")
    } else {
        format!("ALTER DATABASE [{database_name}] SET OFFLINE WITH ROLLBACK IMMEDIATE")
    };
    expect_ok(
        Query::new(sql).execute(&mut client).await,
        "failed to change whether the database is online",
    );
}

pub(crate) async fn is_table_existing(
    connection_string: &str,
    database_name: &str,
    table_name: &str,
) -> bool {
    let mut client = connect(connection_string, Some(database_name)).await;
    let mut query = Query::new("SELECT 1 FROM sysobjects WHERE name = @P1 AND xtype = 'U'");
    query.bind(table_name);
    let row = expect_ok(
        expect_ok(
            query.query(&mut client).await,
            "failed to check table existence",
        )
        .into_row()
        .await,
        "failed to collect table-existence row",
    );
    row.is_some()
}

pub(crate) async fn read_the_user_table_names(
    connection_string: &str,
    database_name: &str,
) -> Vec<String> {
    let mut client = connect(connection_string, Some(database_name)).await;
    let rows = expect_ok(
        expect_ok(
            client
                .simple_query("SELECT name FROM sys.tables ORDER BY name")
                .await,
            "failed to list user tables",
        )
        .into_first_result()
        .await,
        "failed to collect user table rows",
    );

    let mut names = Vec::with_capacity(rows.len());
    for row in rows {
        let name: &str = expect_some(row.get("name"), "missing name column");
        names.push(name.to_string());
    }
    names
}

pub(crate) async fn read_the_tracking_rows(
    connection_string: &str,
    database_name: &str,
) -> Vec<TrackingRow> {
    let mut client = connect(connection_string, Some(database_name)).await;
    let rows = expect_ok(
        expect_ok(
            client
                .simple_query(
                    "SELECT Filename, ExecutedAt, Version FROM DbMigrationsRun ORDER BY Id",
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
        let filename: &str = expect_some(row.get("Filename"), "missing Filename column");
        let executed_at: DateTime<Utc> =
            expect_some(row.get("ExecutedAt"), "missing ExecutedAt column");
        let version: &str = expect_some(row.get("Version"), "missing Version column");
        tracking_rows.push(TrackingRow {
            filename: filename.to_string(),
            executed_at,
            version: version.to_string(),
        });
    }
    tracking_rows
}
