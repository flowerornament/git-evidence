//! The index: a disposable SQLite cache of the folded notes, one file per
//! build of this tool, that every question reads. Before each answer it
//! re-folds only the notes whose tips moved, then the question reads one
//! snapshot under a time limit; deleting the file loses nothing.
//!
//! ```text
//!    notes ──fold changed notes──► index file ──one snapshot──► answer
//! ```
//!
//! [`handle`] opens the file, catches it up and reads it; [`catch_up`]
//! finds and folds what changed; [`note_rows`] turns a note's runs into
//! rows; [`schema`] holds the tables; [`per_build`] names one file per
//! build; [`read`] has the typed column readers the questions share;
//! `fixtures` (tests only) has what the crate's unit tests share.

mod catch_up;
#[cfg(test)]
pub(crate) mod fixtures;
mod handle;
mod note_rows;
mod per_build;
mod read;
mod schema;

pub(crate) use handle::{Cached, pin_in, within};
pub use handle::{Index, IndexError, Pin, Runs, TIME_LIMIT};
pub use note_rows::{DeclaredMeasure, MeasureOrigin};
pub(crate) use note_rows::{direction_from_text, direction_text};
#[cfg(feature = "testing")]
pub use per_build::file_name;
pub(crate) use read::{
    column_values, column_values_with, end_at, machine_at, parsed, tag_at, time_at, unreadable,
};
pub use schema::VIEWS_VERSION;
pub(crate) use schema::landing_date;
