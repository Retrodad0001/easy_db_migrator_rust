use std::str::FromStr;
use std::sync::Once;

use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};
use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;

use super::{expect_ok, sweep_leftover_containers};

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
