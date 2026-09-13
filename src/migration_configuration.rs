use std::path::PathBuf;

use crate::error_kind::ErrorKind;

/// The configuration used to run a migration: how to connect, which database to
/// target, where the migration scripts live, and which scripts to leave out.
#[derive(Debug, Clone)]
pub struct MigrationConfiguration {
    pub(crate) connection_string: String,
    pub(crate) database_name: String,
    pub(crate) scripts_directory: PathBuf,
    pub(crate) excluded_scripts: Vec<String>,
}

impl MigrationConfiguration {
    /// Builds a new configuration, taking the script filenames to exclude from the
    /// run. It refuses a blank connection string, and a database name that is empty
    /// or has more than one word.
    pub fn new(
        connection_string: impl Into<String>,
        database_name: impl Into<String>,
        scripts_directory: impl Into<PathBuf>,
        excluded_scripts: impl IntoIterator<Item = String>,
    ) -> Result<Self, ErrorKind> {
        let connection_string = connection_string.into();
        if connection_string.trim().is_empty() {
            return Err(ErrorKind::InvalidConfig(
                "connection_string cannot be empty or whitespace".to_string(),
            ));
        }

        let database_name = database_name.into();
        if database_name.trim().is_empty() {
            return Err(ErrorKind::InvalidConfig(
                "database_name cannot be empty or whitespace".to_string(),
            ));
        }

        if database_name.split_whitespace().count() > 1 {
            return Err(ErrorKind::InvalidConfig(
                "database_name can only be one word".to_string(),
            ));
        }

        Ok(Self {
            connection_string,
            database_name,
            scripts_directory: scripts_directory.into(),
            excluded_scripts: excluded_scripts.into_iter().collect(),
        })
    }
}
