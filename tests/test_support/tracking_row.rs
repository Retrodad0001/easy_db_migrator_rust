use chrono::{DateTime, Utc};

#[derive(Debug)]
pub(crate) struct TrackingRow {
    pub(crate) filename: String,
    pub(crate) executed_at: DateTime<Utc>,
    pub(crate) version: String,
}
