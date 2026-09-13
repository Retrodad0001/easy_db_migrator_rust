use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use tiberius::{Client, Config, Query};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::{
    CRATE_VERSION, backend::run_migration_result_kind::RunMigrationResultKind,
    error_kind::ErrorKind, migration_configuration::MigrationConfiguration, script::Script,
};

const TRACKING_TABLE: &str = "DbMigrationsRun";

async fn try_connect(
    migration_configuration: &MigrationConfiguration,
    database_name: Option<&str>,
) -> Result<Client<Compat<TcpStream>>, ErrorKind> {
    let mut tds_config = Config::from_ado_string(&migration_configuration.connection_string)
        .map_err(ErrorKind::Mssql)?;
    if let Some(database_name) = database_name {
        tds_config.database(database_name);
    }

    let tcp = TcpStream::connect(tds_config.get_addr()).await?;
    tcp.set_nodelay(true)?;

    let client = Client::connect(tds_config, tcp.compat_write()).await?;
    Ok(client)
}

pub(crate) async fn try_delete_database_if_exists(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut client = try_connect(migration_configuration, None).await?;
    let database_name = &migration_configuration.database_name;
    let query = format!(
        "IF EXISTS (SELECT * FROM sys.databases WHERE name = '{database_name}')
         BEGIN
            ALTER DATABASE [{database_name}] SET OFFLINE WITH ROLLBACK IMMEDIATE;
            ALTER DATABASE [{database_name}] SET ONLINE;
            DROP DATABASE [{database_name}];
         END"
    );
    Query::new(query).execute(&mut client).await?;
    Ok(())
}

pub(crate) async fn try_create_database_if_missing(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut client = try_connect(migration_configuration, None).await?;
    let database_name = &migration_configuration.database_name;
    let query = format!(
        "IF NOT EXISTS (SELECT * FROM sys.databases WHERE name = '{database_name}')
         BEGIN
            CREATE DATABASE [{database_name}]
         END"
    );
    Query::new(query).execute(&mut client).await?;
    Ok(())
}

pub(crate) async fn try_ensure_tracking_table(
    migration_configuration: &MigrationConfiguration,
) -> Result<(), ErrorKind> {
    let mut client = try_connect(
        migration_configuration,
        Some(migration_configuration.database_name.as_str()),
    )
    .await?;
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
    Query::new(query).execute(&mut client).await?;
    Ok(())
}

pub(crate) async fn try_run_script(
    migration_configuration: &MigrationConfiguration,
    script: &Script,
    date_time: DateTime<Utc>,
    is_cancelled: &AtomicBool,
) -> Result<RunMigrationResultKind, ErrorKind> {
    if is_cancelled.load(Ordering::SeqCst) {
        return Ok(RunMigrationResultKind::MigrationWasCancelled);
    }

    let mut client = try_connect(
        migration_configuration,
        Some(migration_configuration.database_name.as_str()),
    )
    .await?;

    let mut already_run_query = Query::new(format!(
        "SELECT Id FROM {TRACKING_TABLE} WHERE Filename = @P1"
    ));
    already_run_query.bind(script.filename.as_str());
    let already_run = already_run_query
        .query(&mut client)
        .await?
        .into_row()
        .await?
        .is_some();

    if already_run {
        return Ok(RunMigrationResultKind::ScriptSkippedBecauseAlreadyRun);
    }

    let run_result = try_run_script_in_transaction(&mut client, script, date_time).await;

    if run_result.is_err() {
        let _ = client
            .simple_query("IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION")
            .await;
    }

    run_result?;
    Ok(RunMigrationResultKind::MigrationScriptExecuted)
}

async fn try_run_script_in_transaction(
    client: &mut Client<Compat<TcpStream>>,
    script: &Script,
    date_time: DateTime<Utc>,
) -> Result<(), ErrorKind> {
    client
        .simple_query("BEGIN TRANSACTION")
        .await?
        .into_results()
        .await?;
    Query::new(script.content.as_str()).execute(client).await?;
    let mut insert_query = Query::new(format!(
        "INSERT INTO {TRACKING_TABLE} (ExecutedAt, Filename, Version) VALUES (@P1, @P2, @P3)"
    ));
    insert_query.bind(date_time);
    insert_query.bind(script.filename.as_str());
    insert_query.bind(CRATE_VERSION);
    insert_query.execute(client).await?;
    client
        .simple_query("COMMIT TRANSACTION")
        .await?
        .into_results()
        .await?;
    Ok(())
}
