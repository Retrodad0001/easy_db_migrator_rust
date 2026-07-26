use chrono::{DateTime, Utc};
use tiberius::{Client, Config, ToSql};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

use crate::{
    CRATE_VERSION, backend::RunMigrationResult, config::MigrationConfiguration, error::Error,
    script::Script,
};

const TRACKING_TABLE: &str = "DbMigrationsRun";

async fn connect(
    config: &MigrationConfiguration,
    database: Option<&str>,
) -> Result<Client<Compat<TcpStream>>, Error> {
    let mut tds_config =
        Config::from_ado_string(config.connection_string()).map_err(Error::Mssql)?;
    if let Some(database) = database {
        tds_config.database(database);
    }

    let tcp = TcpStream::connect(tds_config.get_addr()).await?;
    tcp.set_nodelay(true)?;

    let client = Client::connect(tds_config, tcp.compat_write()).await?;
    Ok(client)
}

pub(crate) async fn try_delete_database_if_exists(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut client = connect(config, None).await?;
    let database_name = config.database_name();
    let query = format!(
        "IF EXISTS (SELECT * FROM sys.databases WHERE name = '{database_name}')
         BEGIN
            ALTER DATABASE [{database_name}] SET OFFLINE WITH ROLLBACK IMMEDIATE;
            ALTER DATABASE [{database_name}] SET ONLINE;
            DROP DATABASE [{database_name}];
         END"
    );
    client.execute(query.as_str(), &[]).await?;
    Ok(())
}

pub(crate) async fn try_create_database_if_missing(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut client = connect(config, None).await?;
    let database_name = config.database_name();
    let query = format!(
        "IF NOT EXISTS (SELECT * FROM sys.databases WHERE name = '{database_name}')
         BEGIN
            CREATE DATABASE [{database_name}]
         END"
    );
    client.execute(query.as_str(), &[]).await?;
    Ok(())
}

pub(crate) async fn try_ensure_tracking_table(
    config: &MigrationConfiguration,
) -> Result<(), Error> {
    let mut client = connect(config, Some(config.database_name())).await?;
    let query = format!(
        "IF NOT EXISTS (SELECT * FROM sysobjects WHERE name = '{TRACKING_TABLE}' AND xtype = 'U')
         BEGIN
            CREATE TABLE {TRACKING_TABLE} (
                Id INT IDENTITY(1,1) PRIMARY KEY,
                ExecutedAt DATETIMEOFFSET NOT NULL,
                Filename NVARCHAR(255) NOT NULL UNIQUE,
                Version NVARCHAR(20) NOT NULL
            )
         END"
    );
    client.execute(query.as_str(), &[]).await?;
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

    let mut client = connect(config, Some(config.database_name())).await?;

    let filename = script.filename();
    let already_run = client
        .query(
            &format!("SELECT Id FROM {TRACKING_TABLE} WHERE Filename = @P1"),
            &[&filename],
        )
        .await?
        .into_row()
        .await?
        .is_some();

    if already_run {
        return Ok(RunMigrationResult::ScriptSkippedBecauseAlreadyRun);
    }

    let version = CRATE_VERSION;
    let run_result = run_script_in_transaction(
        &mut client,
        script.content(),
        filename,
        executed_at,
        version,
    )
    .await;

    if run_result.is_err() {
        let _ = client
            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
            .await;
    }

    run_result?;
    Ok(RunMigrationResult::MigrationScriptExecuted)
}

async fn run_script_in_transaction(
    client: &mut Client<Compat<TcpStream>>,
    script_content: &str,
    filename: &str,
    executed_at: DateTime<Utc>,
    version: &str,
) -> Result<(), Error> {
    client
        .simple_query("BEGIN TRANSACTION")
        .await?
        .into_results()
        .await?;
    client.execute(script_content, &[]).await?;
    client
        .execute(
            &format!("INSERT INTO {TRACKING_TABLE} (ExecutedAt, Filename, Version) VALUES (@P1, @P2, @P3)"),
            &[&executed_at as &dyn ToSql, &filename as &dyn ToSql, &version as &dyn ToSql],
        )
        .await?;
    client
        .simple_query("COMMIT TRANSACTION")
        .await?
        .into_results()
        .await?;
    Ok(())
}
