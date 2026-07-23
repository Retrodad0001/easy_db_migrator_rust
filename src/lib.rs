//! Database migration library for PostgreSQL and SQL Server.
//!
//! Migrations are forward-only: scripts are ordered by the date and sequence number
//! encoded in their filename (`yyyyMMdd_NNN_name.sql`), executed once, and recorded in
//! a `DbMigrationsRun` tracking table so that re-running a migration is a no-op.

mod backend;
mod clock;
mod config;
mod database_kind;
mod error;
mod migrator;
mod script;

pub use clock::{ClockMock, SystemClock};
pub use config::MigrationConfiguration;
pub use database_kind::DatabaseKind;
pub use error::Error;
pub use migrator::DbMigrator;
pub use script::Script;
pub use tokio_util::sync::CancellationToken;

pub(crate) const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
