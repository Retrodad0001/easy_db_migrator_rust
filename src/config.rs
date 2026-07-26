use std::path::{Path, PathBuf};

use crate::error::Error;

/// The configuration used to run a migration: how to connect, which database to
/// target, and where the migration scripts live.
#[derive(Debug, Clone)]
pub struct MigrationConfiguration {
    connection_string: String,
    database_name: String,
    scripts_directory: PathBuf,
}

impl MigrationConfiguration {
    /// Builds a new configuration, validating the database name.
    pub fn new(
        connection_string: impl Into<String>,
        database_name: impl Into<String>,
        scripts_directory: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let connection_string = connection_string.into();
        let database_name = database_name.into();

        if database_name.trim().is_empty() {
            return Err(Error::InvalidConfig(
                "database_name cannot be empty or whitespace".to_string(),
            ));
        }

        if database_name.split_whitespace().count() > 1 {
            return Err(Error::InvalidConfig(
                "database_name can only be one word".to_string(),
            ));
        }

        Ok(Self {
            connection_string,
            database_name,
            scripts_directory: scripts_directory.into(),
        })
    }

    /// The connection string, without a database selected.
    pub fn connection_string(&self) -> &str {
        &self.connection_string
    }

    /// The name of the database this migration run targets.
    pub fn database_name(&self) -> &str {
        &self.database_name
    }

    /// The directory containing the `.sql` migration scripts.
    pub fn scripts_directory(&self) -> &Path {
        &self.scripts_directory
    }
}
