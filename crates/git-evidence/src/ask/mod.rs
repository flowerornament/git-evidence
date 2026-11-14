//! The questions: the overview, `trend`, `log`, `show`, `keys` and `diff`.
//! Each reads the index at one pin through [`crate::Evidence::at`] and
//! returns rows of recorded numbers; none judges them or averages them.
//!
//! ```text
//!    "How did parse_large move lately?"  ──►  one row per run: landing, commit, value
//!    "What ran on this commit?"          ──►  one row per run: kind, how it ended
//! ```
//!
//! A name nothing records (a key, a metric, a run, a commit with no run) is
//! [`NotRecorded`], never an empty answer.

mod diff;
mod keys;
mod landings;
mod log;
mod not_recorded;
mod overview;
mod params_of;
mod selector;
mod show;
pub(crate) mod sql;
mod suggest;
mod trend;

pub(crate) use diff::diff;
pub use diff::{Change, DiffQuery, Difference, RunValue};
pub(crate) use keys::keys;
pub use keys::{KeyInfo, KeysQuery, RecordedMetric};
pub use landings::{Landing, LandingNumber};
pub(crate) use log::log;
pub use log::{LogEntry, LogQuery, Logged};
pub use not_recorded::NotRecorded;
pub(crate) use overview::overview;
pub use overview::{LandingSummary, Overview, RunCount};
pub use selector::{DateError, LandingRange, Selector, UtcDate};
pub(crate) use show::show;
pub use show::{KeyOutcome, Param, RunDetail, ShowQuery, Shown, ShownOutcome};
pub(crate) use trend::trend;
pub use trend::{Point, TrendQuery};
