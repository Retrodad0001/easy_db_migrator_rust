use chrono::NaiveDate;

use crate::error_kind::ErrorKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Script {
    pub(crate) filename: String,
    pub(crate) content: String,
    pub(crate) date_part: NaiveDate,
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
            .map_err(|_| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 4-digit year at position 0".to_string(),
            })?;
        let month: u32 = filename[4..6]
            .parse()
            .map_err(|_| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 2-digit month at position 4".to_string(),
            })?;
        let day: u32 = filename[6..8]
            .parse()
            .map_err(|_| ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 2-digit day at position 6".to_string(),
            })?;
        let date_part = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
            ErrorKind::InvalidScriptName {
                filename: filename.clone(),
                reason: "date part is not a valid calendar date".to_string(),
            }
        })?;

        let sequence_part: u32 =
            filename[9..12]
                .parse()
                .map_err(|_| ErrorKind::InvalidScriptName {
                    filename: filename.clone(),
                    reason: "expected a 3-digit sequence number at position 9".to_string(),
                })?;

        Ok(Self {
            filename,
            content,
            date_part,
            sequence_part,
        })
    }
}
