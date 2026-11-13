//! Writing evidence. `record` checks a batch of lines whole and appends each
//! tree's new lines in one commit ([`batch`]); [`Run`] writes a run event by
//! event, to this store or to a file ([`destination`]), and [`Finish`] writes
//! its rest at once. A refused batch writes nothing and says why
//! ([`refusal`]). The notes are appended through [`crate::store`]; [`fault`]
//! holds the crash tests' abort points.
//!
//! ```text
//!    a benchmark ──┐
//!    a CI step ────┼──►  Evidence::run(...)     ──►  one note line per event, on the commit's tree
//!    a test runner ┘     Evidence::record(...)
//! ```

mod batch;
mod destination;
pub(crate) mod fault;
mod finish;
mod lines;
mod refusal;
mod run;

pub use batch::{Recorded, RunLines};
pub(crate) use batch::{RunLocator, record, record_dry_run};
pub use finish::Finish;
pub use refusal::{RecordError, Refusal};
pub use run::{Resumed, Run};
