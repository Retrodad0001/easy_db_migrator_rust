//! Runs one migration action against a database, so `performancetest.bat` can profile
//! the library.

use easy_db_migrator_rust::DatabaseKind;
use easy_db_migrator_rust::ErrorKind;
use easy_db_migrator_rust::MigrationConfiguration;
use easy_db_migrator_rust::try_apply_migrations;
use easy_db_migrator_rust::try_delete_database_if_exists;
use std::sync::atomic::AtomicBool;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

#[inline]
fn try_determine_the_database_kind(name: &str) -> Result<DatabaseKind, ErrorKind> {
    match name {
        "postgres" => Ok(DatabaseKind::Postgresql),
        "mssql" => Ok(DatabaseKind::Mssql),
        _ => Err(ErrorKind::InvalidConfig(format!(
            "try_determine_the_database_kind found no database named {name}, use postgres or mssql"
        ))),
    }
}

#[tokio::main]
async fn main() -> Result<(), ErrorKind> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    let arguments: Vec<String> = std::env::args().collect();
    let [_, database, action, connection_string, scripts_directory] = arguments.as_slice() else {
        return Err(ErrorKind::InvalidConfig(
            "use profile_migrations <postgres|mssql> <delete|apply> <connection string> <scripts directory>"
                .to_owned(),
        ));
    };
    let database_kind = try_determine_the_database_kind(database)?;
    let migration_configuration = MigrationConfiguration::new(
        connection_string.as_str(),
        "profile_migrations",
        scripts_directory.as_str(),
        Vec::new(),
    )?;
    match action.as_str() {
        "delete" => try_delete_database_if_exists(database_kind, &migration_configuration).await,
        "apply" => {
            try_apply_migrations(
                database_kind,
                &migration_configuration,
                &AtomicBool::new(false),
            )
            .await
        }
        _ => Err(ErrorKind::InvalidConfig(format!(
            "main found no action named {action}, use delete or apply"
        ))),
    }
}
