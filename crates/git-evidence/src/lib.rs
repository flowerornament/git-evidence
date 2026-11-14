//! Numbers recorded per Git commit, kept in Git notes, and read back as rows.
//!
//! The README below is the manual: the model, every verb and column, the
//! settings, and the Rust API by example. The event lines, the notes layout
//! and the public SQL views are specified normatively in
//! [`docs/FORMAT.md`](https://github.com/flowerornament/git-evidence/blob/main/docs/FORMAT.md).
//!
//! The crate root is the whole Rust API, with the `testing` module behind its
//! feature; `tests/contract.rs` pins the exported names.
//!
//! A run is written through one type and read back through others, each
//! shaped for its question:
//!
//! ```text
//! Run         a run being written event by event: opened before the work (Evidence::run)
//! Resumed     a Run found again after a crash, with the keys it had declared and answered
//! Finish      the rest of a Run (declaration, results, close), written in one commit
//! RunLines    one run's share of a record: lines written, already present, its size (Recorded)
//! RunDetail   one run read back whole (show): parameters, end, open, each key's outcome
//! LogEntry    one run listed (log): how it ended and how many keys each outcome has
//! RunCount    how many runs of one kind a landing's tree has, refused ones apart (overview)
//! RunValue    one side of a diff row: the newest run's number, and how many runs had it
//! ```
#![doc = include_str!("../README.md")]

mod ask;
mod evidence;
mod format;
mod index;
mod rows;
mod share;
mod store;
#[cfg(feature = "testing")]
pub mod testing;
mod write;

pub use ask::sql::{PUBLIC_SCHEMA, PUBLIC_VIEWS, Rows, SqlError, SqlValue};
pub use ask::{
    Change, DiffQuery, Difference, KeyInfo, KeyOutcome, KeysQuery, Landing, LandingNumber,
    LandingSummary, LogEntry, LogQuery, Logged, NotRecorded, Overview, Param, Point,
    RecordedMetric, RunCount, RunDetail, RunValue, ShowQuery, Shown, ShownOutcome, TrendQuery,
};
pub use ask::{DateError, LandingRange, Selector, UtcDate};
pub use evidence::{At, Error, Evidence};
pub use format::event::{
    Build, Declaration, Direction, ENVELOPE_VERSION, End, Environment, Event, EventLine, Measure,
    Metric, NotFinite, Open, Optional, Outcome, Producer, Profile, Reason, Suite, Timestamp, Try,
    Verdict, VerdictStatus,
};
pub use format::fold::{Problem, Violation};
pub use format::id::{
    CommitId, IdError, Key, Kind, Machine, MetricName, ObjectId, ProducerName, RunId, Sha256,
    TreeId,
};
pub use index::{DeclaredMeasure, IndexError, MeasureOrigin, Pin};
pub use rows::{TidyRows, condition_leaves};
pub use share::audit::{Audit, Finding, OwnNames, RefAudit};
pub use share::remote::{RefUpdate, RemoteError, Transfer, UpdateKind};
pub use store::StoreError;
pub use write::{Finish, RecordError, Recorded, Refusal, Resumed, Run, RunLines};
