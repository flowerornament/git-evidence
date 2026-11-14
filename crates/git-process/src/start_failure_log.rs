//! The first retried start failure of each test process, written to a
//! directory the test harness names and reads afterwards (feature `testing`).

use crate::{START_FAILURE_DIR, StartFailureRecord, StartObserver};
use std::{fs::OpenOptions, io::Write as _, path::Path, sync::OnceLock};

/// Reports the first retried process-start failure in this process, if the
/// environment names a directory in [`START_FAILURE_DIR`]. Install with
/// `observe_start_failures::<StartFailureLog>()` before any fixture launch.
/// Every executed test worker must initialize itself; exec resets registration.
/// A notification failure is printed to stderr and never replaces the original
/// process result. The absence of a file alone is not proof of zero failures.
pub struct StartFailureLog;

static FIRST_RESOURCE_FAILURE: OnceLock<Result<(), String>> = OnceLock::new();

impl StartObserver for StartFailureLog {
    fn failed(record: &StartFailureRecord) {
        if !record.retrying {
            return;
        }
        let Some(directory) = std::env::var_os(START_FAILURE_DIR) else {
            return;
        };
        let result = FIRST_RESOURCE_FAILURE.get_or_init(|| {
            notify(Path::new(&directory), record).map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            let _ = writeln!(
                std::io::stderr().lock(),
                "process-start notification unavailable: {error}"
            );
        }
    }
}

fn notify(directory: &Path, record: &StartFailureRecord) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    let pending = directory.join(format!("start-failure-{}.pending", record.pid));
    let published = directory.join(format!("start-failure-{}.json", record.pid));
    // A private invocation directory and create-new prevent another worker's
    // diagnostic being overwritten. PID reuse refuses rather than replacing it.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::hard_link(&pending, &published)?;
    std::fs::remove_file(pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logger_uses_the_retry_decision_instead_of_platform_errno_numbers() {
        if std::env::var_os("GIT_PROCESS_LOGGER_TEST_CHILD").is_some() {
            let mut record = StartFailureRecord {
                time: crate::Timestamp::now(),
                pid: std::process::id(),
                attempt: 1,
                // EAGAIN on Linux, deliberately not the macOS number.
                os_error: Some(11),
                error_kind: "display only".into(),
                program: "git".into(),
                retrying: false,
            };
            StartFailureLog::failed(&record);
            let directory = std::env::var_os(START_FAILURE_DIR).expect("private directory");
            assert!(
                !Path::new(&directory)
                    .join(format!("start-failure-{}.json", record.pid))
                    .exists()
            );
            record.retrying = true;
            StartFailureLog::failed(&record);
            return;
        }
        let dir = tempfile::tempdir().expect("private notifications");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child.args(["--exact", "start_failure_log::tests::logger_uses_the_retry_decision_instead_of_platform_errno_numbers", "--nocapture"])
            .env("GIT_PROCESS_LOGGER_TEST_CHILD", "1")
            .env(START_FAILURE_DIR, dir.path());
        assert!(child.status().expect("isolated logger worker").success());
        let files = std::fs::read_dir(dir.path())
            .expect("notifications")
            .collect::<Result<Vec<_>, _>>()
            .expect("entries");
        assert_eq!(files.len(), 1);
        let record: StartFailureRecord =
            serde_json::from_slice(&std::fs::read(files[0].path()).expect("record bytes"))
                .expect("typed record");
        assert_eq!(record.os_error, Some(11));
        assert!(record.retrying);
    }

    #[test]
    fn notification_is_complete_and_never_replaces_an_earlier_pid() {
        let dir = tempfile::tempdir().expect("private diagnostics");
        let record = StartFailureRecord {
            time: crate::Timestamp::now(),
            pid: 123,
            attempt: 1,
            os_error: Some(35),
            error_kind: "WouldBlock".into(),
            program: "git".into(),
            retrying: true,
        };
        notify(dir.path(), &record).expect("notification");
        let bytes =
            std::fs::read(dir.path().join("start-failure-123.json")).expect("complete file");
        let decoded: StartFailureRecord = serde_json::from_slice(&bytes).expect("record");
        assert_eq!(decoded.os_error, Some(35));
        assert_eq!(decoded.attempt, 1);
        assert!(notify(dir.path(), &record).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("start-failure-123.json")).expect("preserved"),
            bytes
        );
        assert!(notify(&dir.path().join("absent"), &record).is_err());
    }
}
