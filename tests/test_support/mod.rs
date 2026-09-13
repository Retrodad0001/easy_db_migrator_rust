#[cfg(feature = "mssql")]
pub(crate) mod mssql;
#[cfg(feature = "postgres")]
pub(crate) mod postgres;
mod tracking_row;

use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use easy_db_migrator_rust::ErrorKind;

pub(crate) use tracking_row::TrackingRow;

macro_rules! assert_logged {
    ($level:expr, $message:expr) => {
        logs_assert(|lines: &[&str]| {
            let expected_level: &str = $level;
            let expected_message: &str = $message;
            let matching: Vec<&&str> = lines
                .iter()
                .filter(|line| line.contains(expected_message))
                .collect();
            if matching.is_empty() {
                return Err(format!(
                    "expected a log record containing {expected_message:?}, found none"
                ));
            }
            if matching
                .iter()
                .any(|line| line.contains(&format!(" {expected_level} ")))
            {
                Ok(())
            } else {
                Err(format!(
                    "expected {expected_message:?} to be logged at {expected_level}, found {matching:?}"
                ))
            }
        });
    };
}

pub(crate) use assert_logged;

pub(crate) fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{context}: {error:?}"),
    }
}

pub(crate) fn expect_migration_error(result: Result<(), ErrorKind>, context: &str) -> String {
    match result {
        Ok(()) => panic!("{context}"),
        Err(error_kind) => error_kind.to_string(),
    }
}

#[cfg(feature = "mssql")]
pub(crate) fn expect_some<T>(value: Option<T>, context: &str) -> T {
    match value {
        Some(value) => value,
        None => panic!("{context}"),
    }
}

#[inline]
pub(crate) fn determine_a_unique_database_name(prefix: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.subsec_nanos());
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);

    format!("{prefix}{}{nanos}{sequence}", std::process::id())
}

pub(crate) fn sweep_leftover_containers(test_label: &str) {
    let listed = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("label={test_label}=1")])
        .output();

    let Ok(listed) = listed else {
        return;
    };
    let Ok(ids) = String::from_utf8(listed.stdout) else {
        return;
    };

    for id in ids.split_whitespace() {
        eprintln!("removing container {id} left behind by an earlier run");
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", id])
            .output();
    }
}
