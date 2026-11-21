use tiberius::{ColumnData, IntoSql, time as tds};
use time::{OffsetDateTime, UtcOffset};

/// The Julian day number of 0001-01-01, the day SQL Server counts the days of a
/// `DATETIMEOFFSET` from.
const JULIAN_DAY_OF_YEAR_ONE: i32 = 1_721_426;

/// The `DATETIMEOFFSET` scale: the time of day is counted in units of 100 nanoseconds.
const SCALE: u8 = 7;
const INCREMENTS_PER_SECOND: u64 = 10_000_000;
const NANOSECONDS_PER_INCREMENT: u32 = 100;

/// The moment a script ran, bound as a SQL Server `DATETIMEOFFSET` parameter.
///
/// tiberius implements only `ToSql` for `OffsetDateTime`, and `Query::bind` needs
/// `IntoSql`, so this type builds the value the way tiberius does for `ToSql`.
pub(crate) struct MssqlExecutedAt(pub(crate) OffsetDateTime);

impl<'a> IntoSql<'a> for MssqlExecutedAt {
    fn into_sql(self) -> ColumnData<'a> {
        let utc = self.0.to_offset(UtcOffset::UTC);
        // a moment from OffsetDateTime::now_utc never lies before the year 1
        let days = utc.date().to_julian_day().abs_diff(JULIAN_DAY_OF_YEAR_ONE);
        let (hour, minute, second, nanosecond) = utc.time().as_hms_nano();
        let seconds = u64::from(hour) * 3_600 + u64::from(minute) * 60 + u64::from(second);
        let increments =
            seconds * INCREMENTS_PER_SECOND + u64::from(nanosecond / NANOSECONDS_PER_INCREMENT);
        let datetime2 =
            tds::DateTime2::new(tds::Date::new(days), tds::Time::new(increments, SCALE));
        ColumnData::DateTimeOffset(Some(tds::DateTimeOffset::new(datetime2, 0)))
    }
}
