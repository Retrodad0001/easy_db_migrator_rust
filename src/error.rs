use std::path::PathBuf;

use thiserror::Error;

/// The single error type returned by all fallible operations in this crate.
#[derive(Debug, Error)]
pub enum Error {
    /// A migration step failed. The message is the same text reported through
    /// `tracing` for that failure.
    #[error("{0}")]
    MigrationFailed(String),

    /// The supplied [`MigrationConfiguration`](crate::MigrationConfiguration) was invalid.
    #[error("invalid migration configuration: {0}")]
    InvalidConfig(String),

    /// A script filename did not match the required `yyyyMMdd_NNN_name.sql` pattern.
    #[error("invalid script filename '{filename}': {reason}")]
    InvalidScriptName {
        /// The offending script filename.
        filename: String,
        /// Why the filename could not be parsed.
        reason: String,
    },

    /// The scripts directory, or a file within it, could not be read.
    #[error("could not read scripts directory '{}': {source}", path.display())]
    ScriptsDirectory {
        /// The directory (or file within it) that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A general I/O error, e.g. establishing a TCP connection to a database server.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An error returned by the PostgreSQL driver.
    #[error("postgres error: {0}")]
    Postgres(#[from] sqlx::Error),

    /// An error returned by the SQL Server driver.
    #[error("mssql error: {0}")]
    Mssql(#[from] tiberius::error::Error),
}
