use std::sync::Once;

use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{expect_ok, sweep_leftover_containers};

pub(crate) async fn start_the_container() -> (ContainerAsync<GenericImage>, String) {
    const TEST_LABEL: &str = "easy_db_migrator_rust_mssql_test";
    static SWEPT: Once = Once::new();
    SWEPT.call_once(|| sweep_leftover_containers(TEST_LABEL));

    const SA_PASSWORD: &str = "yourStrong(!)Password";
    let container = expect_ok(
        GenericImage::new("mcr.microsoft.com/mssql/server", "2022-CU27-ubuntu-22.04")
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
