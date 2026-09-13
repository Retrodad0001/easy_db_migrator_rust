//! Database migration library for PostgreSQL and SQL Server.
//!
//! Each backend is a Cargo feature: `postgres` and `mssql`. There is no default
//! feature, so a consumer names the backend it wants and the other backend's
//! driver stays out of the build entirely. Naming both is allowed; naming
//! neither is a build error.

#[cfg(not(any(feature = "postgres", feature = "mssql")))]
compile_error!(
    "easy_db_migrator_rust needs at least one backend feature enabled: \"postgres\", \"mssql\", or both"
);

mod backend;
mod database_kind;
mod error_kind;
mod migration_configuration;
mod migrator;
mod run_outcome_kind;
mod script;

pub use database_kind::DatabaseKind;
pub use error_kind::ErrorKind;
pub use migration_configuration::MigrationConfiguration;
pub use migrator::{try_apply_migrations, try_delete_database_if_exists};

pub(crate) const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
