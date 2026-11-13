//! Why a write was refused or failed: [`RecordError`], and [`Refusal`], the
//! rule a batch would break with what to do instead.

use crate::format::fold::Problem;
use crate::format::id::{CommitId, Kind, RunId, TreeId};
use crate::store::StoreError;

/// A refused or failed record.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RecordError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] git_process::GitError),
    #[error("cannot encode an event: {0}")]
    Encode(#[from] serde_json::Error),
    /// The batch would break a rule; nothing was written.
    #[error("refused: {0}")]
    Refused(#[from] Refusal),
    /// Where a run is recorded could not be looked up.
    #[error("cannot look up where a run is recorded: {0}")]
    Locate(#[source] rusqlite::Error),
    /// A returned run's file could not be written.
    #[error("cannot write the returned run: {}: {source}", path.display())]
    Returned {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Why `record` refused a batch: the rule it would break and how to write it
/// instead. A refused batch writes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A line of another envelope version.
    Envelope { run: RunId, found: u32 },
    /// A time not in the one sortable form.
    Time { run: RunId, seq: u32, time: String },
    /// A NaN or infinity, which JSON cannot hold.
    NonFinite { run: RunId, seq: u32, field: String },
    /// A negative wall or CPU time in a verdict try, which no try can take.
    Negative { run: RunId, seq: u32, field: String },
    /// An open naming a commit this store does not have.
    UnknownCommit { run: RunId, commit: CommitId },
    /// An open for a run already recorded on another tree.
    OtherTree { run: RunId, recorded: TreeId },
    /// Later events of a run whose open is neither in the batch nor recorded.
    NoOpen { run: RunId },
    /// A run to resume that this store does not hold.
    NotRecorded { run: RunId },
    /// A run to resume that has ended.
    Ended { run: RunId },
    /// A batch that would take its run over the kind's size budget.
    OverBudget {
        run: RunId,
        kind: Kind,
        budget: u64,
        bytes: u64,
    },
    /// A line or run that would break the run grammar or its metric declarations.
    Rule {
        run: Option<RunId>,
        problem: Problem,
    },
}

impl Refusal {
    /// What to do instead.
    #[must_use]
    pub const fn remedy(&self) -> &'static str {
        match self {
            Self::Envelope { .. } => "write version 2 event lines",
            Self::Time { .. } => "write times as YYYY-MM-DDTHH:MM:SS.mmmZ, in UTC",
            Self::NonFinite { .. } => {
                "record the key refused with a reason, or leave the number out"
            }
            Self::Negative { .. } => "record the time the try took, or leave it out",
            Self::UnknownCommit { .. } => "fetch the commit first",
            Self::OtherTree { .. } => "a run is about one tree: open a new run",
            Self::NoOpen { .. } => "send the run's open first",
            Self::NotRecorded { .. } => "check the run id, or fetch the store that recorded it",
            Self::Ended { .. } => "a run ends once: open a new run",
            Self::OverBudget { .. } => {
                "record fewer lines in the run, or raise git config --local evidence.<kind>.budget"
            }
            Self::Rule { problem, .. } => problem_remedy(problem),
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Envelope { run, found } => write!(f, "run {run}: envelope version {found}"),
            Self::Time { run, seq, time } => {
                write!(
                    f,
                    "run {run} seq {seq}: time {time:?} is not YYYY-MM-DDTHH:MM:SS.mmmZ"
                )
            }
            Self::NonFinite { run, seq, field } => write!(
                f,
                "run {run} seq {seq}: {field} is not a finite number (JSON has no NaN or infinity)"
            ),
            Self::Negative { run, seq, field } => {
                write!(f, "run {run} seq {seq}: {field} is negative")
            }
            Self::UnknownCommit { run, commit } => {
                write!(f, "run {run}: commit {commit} is not in this store")
            }
            Self::OtherTree { run, recorded } => {
                write!(f, "run {run} is already recorded on tree {recorded}")
            }
            Self::NoOpen { run } => {
                write!(
                    f,
                    "run {run}: no open in this batch, and the run is not recorded"
                )
            }
            Self::NotRecorded { run } => write!(f, "run {run} is not recorded in this store"),
            Self::Ended { run } => write!(f, "run {run} has ended"),
            Self::OverBudget {
                run,
                kind,
                budget,
                bytes,
            } => write!(
                f,
                "run {run} would be {bytes} bytes; the {kind} budget is {budget}"
            ),
            Self::Rule {
                run: Some(run),
                problem,
            } => write!(f, "run {run}: {problem}"),
            Self::Rule { run: None, problem } => write!(f, "{problem}"),
        }?;
        write!(f, "; {}", self.remedy())
    }
}

impl std::error::Error for Refusal {}

/// How to write a run that breaks `problem` correctly instead.
const fn problem_remedy(problem: &Problem) -> &'static str {
    match problem {
        Problem::UndeclaredMetric { .. } => {
            "declare the metric in suite.metrics at open, or in declare.metrics before the result"
        }
        Problem::MetricRedefined { .. } => {
            "declare each metric once, with one unit and direction; a new meaning is a new metric name"
        }
        Problem::ReservedMetric { .. } => {
            "the index derives verdict.* times from verdict tries; record the verdict with its tries"
        }
        Problem::NoUnit { .. } => {
            "declare the metric's UCUM unit (`ns`, `ms`, `By`, `1`, `{count}`)"
        }
        Problem::IncompleteClose { .. } => {
            "answer every declared key, or close with end refused and a reason"
        }
        Problem::UndeclaredKey { .. } => "declare the key at open or in the run's one declare",
        Problem::SecondResult { .. } => "each key takes one result",
        Problem::AfterClose { .. } => "nothing follows close; open a new run",
        Problem::SecondOpen { .. }
        | Problem::FirstNotOpen
        | Problem::FirstMissing
        | Problem::Empty => "a run is open · declare? · result* · close?, numbered from seq 0",
        Problem::DeclareAfterResult { .. } => {
            "declare keys and metrics before the run's first result, or open a new run"
        }
        Problem::SecondDeclare { .. } | Problem::KeysTwice { .. } => {
            "declare keys and metrics once per run"
        }
        Problem::Gap { .. } | Problem::ConflictingSeq { .. } => {
            "number a run's events from 0 without gaps, each seq once"
        }
        Problem::Time { .. } => "write times as YYYY-MM-DDTHH:MM:SS.mmmZ",
        Problem::Machine(_) => {
            "name the machine as git config evidence.machine does (no whitespace, `/` or control characters), or leave it empty"
        }
        Problem::Unreadable(_) | Problem::Envelope { .. } => {
            "write one JSON event per line in the v2 format"
        }
    }
}
