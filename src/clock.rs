use chrono::{DateTime, Utc};

/// Supplies the current time to [`DbMigrator`](crate::DbMigrator) when stamping
/// tracking-table rows.
///
/// Swappable so integration tests can inject a fixed time and assert on it, the way
/// the ported `.NET` contract does with `IDateTimeHelper`. The `Mock` suffix marks
/// this as a trait that exists solely to let tests substitute a fake — not a
/// general-purpose dispatch mechanism (see the crate's `Match over traits` rule).
pub trait ClockMock: Send + Sync {
    /// Returns the current UTC time.
    fn now_utc(&self) -> DateTime<Utc>;
}

/// The default [`ClockMock`], backed by the system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl ClockMock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
