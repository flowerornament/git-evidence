//! A caller's observer survives the test fixture's setup and sees real failed
//! starts. Keep this binary to one test: observer registration is
//! process-global.
use git_process::{StartFailureRecord, StartObserver};
use std::{process::Command, sync::Mutex};
static RECORDS: Mutex<Vec<StartFailureRecord>> = Mutex::new(Vec::new());
struct Caller;
impl StartObserver for Caller {
    fn failed(record: &StartFailureRecord) {
        RECORDS.lock().expect("observations").push(record.clone());
    }
}
struct Other;
impl StartObserver for Other {
    fn failed(_: &StartFailureRecord) {}
}

#[test]
fn caller_observer_is_preserved_and_actual_start_failures_reach_it() {
    git_process::observe_start_failures::<Caller>().expect("first observer");
    git_process::observe_start_failures::<Caller>().expect("idempotent");
    assert!(git_process::observe_start_failures::<Other>().is_err());
    // The fixture installs its own observer only where none is installed.
    let _repo = git_process::testing::repo();
    RECORDS.lock().expect("observations").clear();
    let absent = tempfile::tempdir().expect("empty executable directory");
    let program = absent.path().join("no-such-program");
    assert!(
        matches!(git_process::start(&mut Command::new(&program)), Err(git_process::GitError::Spawn(ref error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    let records = RECORDS.lock().expect("observations");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].attempt, 1);
    assert!(!records[0].retrying);
    assert_eq!(records[0].pid, std::process::id());
    assert_eq!(records[0].program, program.to_string_lossy());
}
