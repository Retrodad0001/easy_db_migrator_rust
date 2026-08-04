//! Database migration library for PostgreSQL and SQL Server.
//!
//! Each backend is a Cargo feature: `postgres` and `mssql`, both enabled by
//! default. A consumer that needs only one takes it with `default-features =
//! false`, which leaves the other backend's driver out of the build entirely.
//! At least one of the two must be enabled.

#[cfg(not(any(feature = "postgres", feature = "mssql")))]
compile_error!(
    "easy_db_migrator_rust needs at least one backend feature enabled: \"postgres\", \"mssql\", or both"
);

mod backend;
mod cancellation;
mod clock;
mod config;
mod database_kind;
mod error;
mod migrator;
mod script;

pub use cancellation::CancellationToken;
pub use clock::{ClockMock, SystemClock};
pub use config::MigrationConfiguration;
pub use database_kind::DatabaseKind;
pub use error::Error;
pub use migrator::DbMigrator;
pub use script::Script;

pub(crate) const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
