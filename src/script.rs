use time::{Date, Month};

use crate::error_kind::ErrorKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Script {
    pub(crate) filename: String,
    pub(crate) content: String,
    pub(crate) date_part: Date,
    pub(crate) sequence_part: u32,
}

impl Script {
    pub(crate) fn new(filename: String, content: String) -> Result<Self, ErrorKind> {
        if filename.trim().is_empty() {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "filename cannot be empty".to_string(),
            });
        }

        if content.trim().is_empty() {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "content cannot be empty".to_string(),
            });
        }

        let Some(prefix) = filename.as_bytes().get(..12) else {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        };
        if !prefix.is_ascii() {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        }

        let Some(&separator) = prefix.get(8) else {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        };
        if separator != b'_' {
            return Err(ErrorKind::InvalidScriptName {
                filename,
                reason: "expected '_' separator after the date part".to_string(),
            });
        }

        let year: i32 = filename[0..4]
            .parse()
            .map_err(|error| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: format!("expected a 4-digit year at position 0: {error}"),
            })?;
        let month: u8 = filename[4..6]
            .parse()
            .map_err(|error| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: format!("expected a 2-digit month at position 4: {error}"),
            })?;
        let day: u8 = filename[6..8]
            .parse()
            .map_err(|error| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: format!("expected a 2-digit day at position 6: {error}"),
            })?;
        let date_part = Month::try_from(month)
            .and_then(|month| Date::from_calendar_date(year, month, day))
            .ok()
            .ok_or_else(|| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: "date part is not a valid calendar date".to_string(),
            })?;

        let sequence_part: u32 =
            filename[9..12]
                .parse()
                .map_err(|error| ErrorKind::InvalidScriptName {
                    filename: filename.clone(),
                    reason: format!("expected a 3-digit sequence number at position 9: {error}"),
                })?;

        Ok(Self {
            filename,
            content,
            date_part,
            sequence_part,
        })
    }
}

#[cfg(test)]
mod tests {
    use time::Month;

    use super::Script;
    use crate::error_kind::ErrorKind;

    const CONTENT: &str = "CREATE TABLE customers (id INTEGER PRIMARY KEY);";
    const NAME_PATTERN: &str = "expected 'yyyyMMdd_NNN_name.sql'";
    const NOT_A_DATE: &str = "date part is not a valid calendar date";

    fn new_script(filename: &str) -> Result<Script, ErrorKind> {
        Script::new(filename.to_string(), CONTENT.to_string())
    }

    fn reason_of(result: Result<Script, ErrorKind>) -> Option<String> {
        let Err(ErrorKind::InvalidScriptName { reason, .. }) = result else {
            return None;
        };
        Some(reason)
    }

    fn date_and_sequence_of(result: Result<Script, ErrorKind>) -> Option<(i32, Month, u8, u32)> {
        let Ok(script) = result else {
            return None;
        };
        let (year, month, day) = script.date_part.to_calendar_date();
        Some((year, month, day, script.sequence_part))
    }

    #[test]
    fn test_given_valid_name_when_script_is_created_then_date_and_sequence_are_read() {
        // What it asserts.
        // 20211230_002_Script2p.sql gives the date 2021-12-30 and the sequence 2.
        assert_eq!(
            date_and_sequence_of(new_script("20211230_002_Script2p.sql")),
            Some((2021, Month::December, 30, 2)),
            "the date and the sequence should be read from the name, and they are not"
        );
    }

    #[test]
    fn test_given_leap_day_when_script_is_created_then_it_is_accepted_only_in_a_leap_year() {
        // What it asserts.
        // 20240229 is a valid date, because 2024 is a leap year, and 20230229 is not.
        assert_eq!(
            date_and_sequence_of(new_script("20240229_001_leap.sql")),
            Some((2024, Month::February, 29, 1)),
            "29 February 2024 should be accepted, and it is not"
        );
        assert_eq!(
            reason_of(new_script("20230229_001_leap.sql")).as_deref(),
            Some(NOT_A_DATE),
            "29 February 2023 should be refused as no calendar date, and it is not"
        );
    }

    #[test]
    fn test_given_impossible_dates_when_script_is_created_then_they_are_refused() {
        // What it asserts.
        // Month 00, month 13, day 00 and 30 February are each refused as no
        // calendar date.
        for filename in [
            "20210030_001_x.sql",
            "20211330_001_x.sql",
            "20211200_001_x.sql",
            "20210230_001_x.sql",
        ] {
            assert_eq!(
                reason_of(new_script(filename)).as_deref(),
                Some(NOT_A_DATE),
                "{filename} should be refused as no calendar date, and it is not"
            );
        }
    }

    #[test]
    fn test_given_empty_filename_or_content_when_script_is_created_then_it_is_refused() {
        // What it asserts.
        // A blank filename and blank content are each refused with their own reason.
        assert_eq!(
            reason_of(new_script("  ")).as_deref(),
            Some("filename cannot be empty"),
            "a blank filename should be refused, and it is not"
        );
        assert_eq!(
            reason_of(Script::new(
                "20211230_002_Script2p.sql".to_string(),
                " \n ".to_string()
            ))
            .as_deref(),
            Some("content cannot be empty"),
            "blank content should be refused, and it is not"
        );
    }

    #[test]
    fn test_given_malformed_prefix_when_script_is_created_then_the_pattern_is_named() {
        // What it asserts.
        // A name shorter than its 12-character prefix, and a prefix with a non-ASCII
        // character, are refused with the expected name pattern.
        for filename in ["20211230_01", "2021123\u{e9}_001_x.sql"] {
            assert_eq!(
                reason_of(new_script(filename)).as_deref(),
                Some(NAME_PATTERN),
                "{filename} should be refused with the name pattern, and it is not"
            );
        }
    }

    #[test]
    fn test_given_wrong_separator_when_script_is_created_then_it_is_refused() {
        // What it asserts.
        // A '-' where the '_' after the date part belongs is refused.
        assert_eq!(
            reason_of(new_script("20211230-002_Script2p.sql")).as_deref(),
            Some("expected '_' separator after the date part"),
            "a wrong separator should be refused, and it is not"
        );
    }

    #[test]
    fn test_given_non_digit_part_when_script_is_created_then_that_part_is_named() {
        // What it asserts.
        // A letter in the year, the month, the day or the sequence number is refused
        // with a reason that names that part and its position.
        for (filename, expected_start) in [
            (
                "2o21l230_002_x.sql",
                "expected a 4-digit year at position 0",
            ),
            (
                "2021x230_002_x.sql",
                "expected a 2-digit month at position 4",
            ),
            ("202112x0_002_x.sql", "expected a 2-digit day at position 6"),
            (
                "20211230_0x2_x.sql",
                "expected a 3-digit sequence number at position 9",
            ),
        ] {
            let reason = reason_of(new_script(filename));
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with(expected_start)),
                "{filename} should be refused with '{expected_start}', and it is refused with {reason:?}"
            );
        }
    }
}
