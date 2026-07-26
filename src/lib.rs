//! Database migration library for PostgreSQL and SQL Server.

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
