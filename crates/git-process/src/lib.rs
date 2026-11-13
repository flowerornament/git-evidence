#![doc = include_str!("../README.md")]

mod git;
mod id;
mod start;
#[cfg(any(test, feature = "testing"))]
mod start_failure_log;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod time;

pub use git::{Git, GitError, TreeEntry, isolated};
pub use id::{CommitId, IdError, ObjectId, TreeId};
pub use start::{
    AlreadyObserved, START_FAILURE_DIR, StartFailureRecord, StartObserver, observe_start_failures,
    output_of, start,
};
#[cfg(any(test, feature = "testing"))]
pub use start_failure_log::StartFailureLog;
pub use time::{NotATimestamp, Timestamp};
