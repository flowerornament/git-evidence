//! Process starts and their failed attempts. A start the OS refused ran
//! nothing, so it is retried; every failed attempt is reported to the one
//! observer a process may install, before any wait or refusal. This module
//! reads no environment and writes no diagnostics itself; `StartFailureLog`
//! (feature `testing`) is an observer that writes failures to a directory a
//! test harness names.

use crate::GitError;
use std::{
    any::TypeId,
    io,
    process::{Child, Command, Output, Stdio},
    sync::OnceLock,
};

/// One unsuccessful OS process start, observed before any retry or return.
/// Program text is diagnostic replacement decoding, not an exact command or
/// executable identity. Arguments and environment are deliberately absent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StartFailureRecord {
    pub time: crate::Timestamp,
    pub pid: u32,
    pub attempt: usize,
    pub os_error: Option<i32>,
    /// Display only; readers must not treat Debug spellings as policy tags.
    pub error_kind: String,
    pub program: String,
    pub retrying: bool,
}

impl StartFailureRecord {
    /// The record of a start of `program` that failed with `error`, now and in
    /// this process: what a launcher outside [`start`] writes, so every failed
    /// start reads the same. `attempt` counts from 1; `retrying` says whether
    /// another attempt follows.
    #[must_use]
    pub fn new(
        program: impl Into<String>,
        error: &io::Error,
        attempt: usize,
        retrying: bool,
    ) -> Self {
        Self {
            time: crate::Timestamp::now(),
            pid: std::process::id(),
            attempt,
            os_error: error.raw_os_error(),
            error_kind: format!("{:?}", error.kind()),
            program: program.into(),
            retrying,
        }
    }
}

/// The environment variable that names the directory a `StartFailureLog`
/// (feature `testing`) writes to. A test harness, built without the feature,
/// sets it for the test processes it starts and reads the directory when they
/// are done: each process that had a start retried writes its first such
/// failure there, as one JSON [`StartFailureRecord`] in
/// `start-failure-<pid>.json`. Unset, the log writes nothing.
pub const START_FAILURE_DIR: &str = "GIT_PROCESS_START_FAILURE_DIR";

/// A process-local consumer of failed starts. Called synchronously before
/// sleeping or refusing; it must not panic, recurse into this module, or block
/// waiting for a child. Registration does not survive an exec.
pub trait StartObserver: 'static {
    fn failed(record: &StartFailureRecord);
}

/// A different observer already owns this process's failure observations.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("a different process-start observer is already registered")]
pub struct AlreadyObserved;

struct Observer {
    identity: TypeId,
    failed: fn(&StartFailureRecord),
}

static OBSERVER: OnceLock<Observer> = OnceLock::new();

/// Installs one observer for this process; registering the same type is
/// idempotent. No observer is installed by default.
///
/// # Errors
/// [`AlreadyObserved`] if a different type was registered first.
pub fn observe_start_failures<O: StartObserver>() -> Result<(), AlreadyObserved> {
    register::<O>(&OBSERVER)
}

fn register<O: StartObserver>(slot: &OnceLock<Observer>) -> Result<(), AlreadyObserved> {
    let observer = slot.get_or_init(|| Observer {
        identity: TypeId::of::<O>(),
        failed: O::failed,
    });
    if observer.identity == TypeId::of::<O>() {
        Ok(())
    } else {
        Err(AlreadyObserved)
    }
}

/// Starts a prepared command. A failed start ran nothing, so retry after
/// 20, 80 and 320 ms; missing or forbidden programs and oversized argument
/// vectors refuse immediately.
/// Observer notification precedes every sleep and the final refusal, including
/// attempts followed by a successful retry. Observation never changes backoff.
///
/// # Errors
/// [`GitError::Spawn`] when the process never started.
pub fn start(command: &mut Command) -> Result<Child, GitError> {
    attempts(command, Command::spawn, OBSERVER.get())
}

fn attempts(
    command: &mut Command,
    mut spawn: impl FnMut(&mut Command) -> io::Result<Child>,
    observer: Option<&Observer>,
) -> Result<Child, GitError> {
    const BACKOFF_MS: [u64; 3] = [20, 80, 320];
    let mut waits = BACKOFF_MS.iter();
    loop {
        match spawn(command) {
            Ok(child) => return Ok(child),
            Err(error) => {
                let attempt = BACKOFF_MS.len() - waits.len() + 1;
                let wait = if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::PermissionDenied
                        | io::ErrorKind::ArgumentListTooLong
                ) {
                    None
                } else {
                    waits.next()
                };
                if let Some(observer) = observer {
                    (observer.failed)(&StartFailureRecord::new(
                        command.get_program().to_string_lossy(),
                        &error,
                        attempt,
                        wait.is_some(),
                    ));
                }
                let Some(wait) = wait else {
                    return Err(GitError::Spawn(error));
                };
                std::thread::sleep(std::time::Duration::from_millis(*wait));
            }
        }
    }
}

/// Captures a command's standard output and error through [`start`].
///
/// # Errors
/// [`GitError::Spawn`] when it never started; [`GitError::Io`] when reading
/// its output or waiting for it failed.
pub fn output_of(command: &mut Command) -> Result<Output, GitError> {
    start(command.stdout(Stdio::piped()).stderr(Stdio::piped()))?
        .wait_with_output()
        .map_err(GitError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static RECORDS: Mutex<Vec<StartFailureRecord>> = Mutex::new(Vec::new());
    struct Collect;
    impl StartObserver for Collect {
        fn failed(record: &StartFailureRecord) {
            RECORDS.lock().expect("records").push(record.clone());
        }
    }
    struct Other;
    impl StartObserver for Other {
        fn failed(_: &StartFailureRecord) {}
    }

    #[test]
    fn a_record_built_outside_start_reads_like_one_start_writes() {
        let error = io::Error::from_raw_os_error(22);
        let record = StartFailureRecord::new("git", &error, 2, true);
        assert_eq!(record.pid, std::process::id());
        assert_eq!(
            (record.program.as_str(), record.attempt, record.retrying),
            ("git", 2, true)
        );
        assert_eq!(record.os_error, Some(22));
        assert_eq!(record.error_kind, format!("{:?}", error.kind()));
    }

    #[test]
    fn every_failed_attempt_is_observed_even_when_the_next_attempt_recovers() {
        let slot = OnceLock::new();
        register::<Collect>(&slot).expect("first observer");
        register::<Collect>(&slot).expect("same observer");
        assert!(register::<Other>(&slot).is_err());
        let mut calls = 0;
        let mut command = Command::new("unused-test-program");
        let refused = attempts(
            &mut command,
            |_| {
                calls += 1;
                Err(io::Error::from_raw_os_error(22))
            },
            slot.get(),
        );
        assert!(
            matches!(refused, Err(GitError::Spawn(ref error)) if error.raw_os_error() == Some(22))
        );
        assert_eq!(calls, 4);
        let records = std::mem::take(&mut *RECORDS.lock().expect("records"));
        assert_eq!(
            records
                .iter()
                .map(|r| (r.attempt, r.retrying))
                .collect::<Vec<_>>(),
            [(1, true), (2, true), (3, true), (4, false)]
        );
        assert!(records.iter().all(|r| r.os_error == Some(22)));
        calls = 0;
        let mut child = attempts(
            &mut command,
            |_| {
                calls += 1;
                if calls == 1 {
                    Err(io::Error::from_raw_os_error(22))
                } else {
                    Command::new("git")
                        .arg("--version")
                        .stdout(Stdio::null())
                        .spawn()
                }
            },
            slot.get(),
        )
        .expect("second attempt recovers");
        assert!(child.wait().expect("wait").success());
        let records = std::mem::take(&mut *RECORDS.lock().expect("records"));
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].attempt, records[0].retrying), (1, true));
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::ArgumentListTooLong,
        ] {
            calls = 0;
            let refused = attempts(
                &mut command,
                |_| {
                    calls += 1;
                    Err(io::Error::from(kind))
                },
                slot.get(),
            );
            assert!(matches!(refused, Err(GitError::Spawn(ref error)) if error.kind() == kind));
            assert_eq!(calls, 1, "unchanged input cannot repair {kind:?}");
            let records = std::mem::take(&mut *RECORDS.lock().expect("records"));
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].os_error, None);
            assert!(!records[0].retrying);
        }
    }

    /// The real launch of a missing program is the error a retry cannot fix:
    /// it is tried once, counted by its spawns, not by how long it took.
    #[test]
    fn a_missing_program_is_refused_at_once_and_never_retried() {
        let mut spawns = 0;
        let refused = attempts(
            &mut Command::new("git-process-no-such-program"),
            |command| {
                spawns += 1;
                command.spawn()
            },
            None,
        );
        assert!(
            matches!(refused, Err(GitError::Spawn(ref e)) if e.kind() == io::ErrorKind::NotFound)
        );
        assert_eq!(spawns, 1, "no retry for an error a retry cannot fix");
    }
}
