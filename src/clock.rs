use chrono::{DateTime, Utc};

/// Supplies the current time to [`DbMigrator`](crate::DbMigrator) when stamping
/// tracking-table rows.
pub trait ClockMock: Send + Sync {
    /// Returns the current UTC time.
    fn now_utc(&self) -> DateTime<Utc>;
}

/// The default [`ClockMock`], backed by the system clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl ClockMock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
