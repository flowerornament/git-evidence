//! What a question named that the store has nothing recorded for: a key, a
//! metric, a run, a commit. It is neither an empty answer (the name exists,
//! no row matched) nor a failure (the store could not be read); the command
//! line prints it on standard error and exits 2.

use crate::format::id::{CommitId, RunId, TreeId};

/// A name the store has nothing recorded for, with the nearest recorded names
/// (at most three, nearest first; empty when nothing of the sort is recorded).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotRecorded {
    /// No run records this key.
    #[error(
        "no run records key {key:?}; {}",
        nearest_names(nearest, "no key is recorded")
    )]
    Key { key: String, nearest: Vec<String> },
    /// The key was recorded, never with this metric.
    #[error(
        "{key} was never recorded with metric {metric:?}; {}",
        nearest_names(nearest, "it has no metric")
    )]
    Metric {
        key: String,
        metric: String,
        nearest: Vec<String>,
    },
    /// No run of this kind is recorded.
    #[error(
        "no run of kind {kind:?} is recorded; {}",
        nearest_names(nearest, "no kind is recorded")
    )]
    Kind { kind: String, nearest: Vec<String> },
    /// No recorded metric matches this glob.
    #[error(
        "no recorded metric matches {glob:?}; {}",
        nearest_names(nearest, "no metric is recorded")
    )]
    MetricGlob { glob: String, nearest: Vec<String> },
    /// No run has this id.
    #[error("no run {0} is recorded")]
    Run(RunId),
    /// The run was recorded, but its lines could not be read; `audit` lists them.
    #[error("run {run} is recorded but could not be read: {problem}")]
    Unreadable { run: RunId, problem: String },
    /// No run was recorded on this tree.
    #[error("nothing is recorded on tree {0}")]
    Tree(TreeId),
    /// A range end that is not on the trunk's first-parent line.
    #[error("{0} is not a landing on the trunk: name landings on both sides of A..B")]
    NotALanding(CommitId),
}

/// `nearest: a, b, c`, or `none` when there is no name to offer.
fn nearest_names(names: &[String], none: &str) -> String {
    if names.is_empty() {
        none.to_owned()
    } else {
        format!("nearest: {}", names.join(", "))
    }
}
