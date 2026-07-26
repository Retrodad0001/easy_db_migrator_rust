use std::path::Path;

use chrono::NaiveDate;

use crate::error::Error;

/// A single migration script loaded from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    filename: String,
    content: String,
    date_part: NaiveDate,
    sequence_part: u32,
}

impl Script {
    /// Parses a script from its filename (`yyyyMMdd_NNN_name.sql`) and content.
    pub fn parse(filename: impl Into<String>, content: impl Into<String>) -> Result<Self, Error> {
        let filename = filename.into();
        let content = content.into();

        if filename.trim().is_empty() {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "filename cannot be empty".to_string(),
            });
        }

        if content.trim().is_empty() {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "content cannot be empty".to_string(),
            });
        }

        let Some(prefix) = filename.as_bytes().get(..12) else {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        };
        if !prefix.is_ascii() {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        }

        let Some(&separator) = prefix.get(8) else {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "expected 'yyyyMMdd_NNN_name.sql'".to_string(),
            });
        };
        if separator != b'_' {
            return Err(Error::InvalidScriptName {
                filename,
                reason: "expected '_' separator after the date part".to_string(),
            });
        }

        let year: i32 = filename[0..4]
            .parse()
            .map_err(|_| Error::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 4-digit year at position 0".to_string(),
            })?;
        let month: u32 = filename[4..6]
            .parse()
            .map_err(|_| Error::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 2-digit month at position 4".to_string(),
            })?;
        let day: u32 = filename[6..8]
            .parse()
            .map_err(|_| Error::InvalidScriptName {
                filename: filename.clone(),
                reason: "expected a 2-digit day at position 6".to_string(),
            })?;
        let date_part =
            NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| Error::InvalidScriptName {
                filename: filename.clone(),
                reason: "date part is not a valid calendar date".to_string(),
            })?;

        let sequence_part: u32 = filename[9..12]
            .parse()
            .map_err(|_| Error::InvalidScriptName {
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

    /// The script's filename, used as its unique identity in the tracking table.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// The raw SQL content of the script.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// The date encoded in the filename, used for ordering.
    pub fn date_part(&self) -> NaiveDate {
        self.date_part
    }

    /// The sequence number encoded in the filename, used for ordering within a date.
    pub fn sequence_part(&self) -> u32 {
        self.sequence_part
    }
}

pub(crate) fn load_ordered_scripts(
    directory: &Path,
    excluded: &[String],
) -> Result<Vec<Script>, Error> {
    let entries = std::fs::read_dir(directory).map_err(|source| Error::ScriptsDirectory {
        path: directory.to_path_buf(),
        source,
    })?;

    let mut scripts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::ScriptsDirectory {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if excluded
            .iter()
            .any(|excluded_name| excluded_name == &filename)
        {
            continue;
        }

        let content = std::fs::read_to_string(&path).map_err(|source| Error::ScriptsDirectory {
            path: path.clone(),
            source,
        })?;

        scripts.push(Script::parse(filename, content)?);
    }

    scripts.sort_by(|a, b| {
        a.date_part
            .cmp(&b.date_part)
            .then(a.sequence_part.cmp(&b.sequence_part))
    });
    Ok(scripts)
}
