//! A run written event by event, for producers that stream: opened before
//! the work, so a crash leaves a visible unfinished run, never a silent gap;
//! found again after a crash; and finished in one commit.

use super::batch::{OpensOnly, RunLocator, record, record_dry_run};
use super::destination::{Destination, replace_file};
use super::{Finish, RecordError, Refusal};
use crate::format::event::{
    Build, ENVELOPE_VERSION, End, Event, EventLine, Metric, Open, Outcome, Timestamp,
};
use crate::format::fold::{Problem, fold};
use crate::format::id::{Key, Kind, MetricName, RunId, TreeId};
use crate::store::Store;
use std::collections::{BTreeMap, BTreeSet};

/// A run being written event by event, for producers that stream. Each call is
/// one durable write: into this store, or, for a run returned to the store of
/// whoever asked for it, into the run's file.
#[derive(Debug)]
pub struct Run {
    pub(super) store: Store,
    pub(super) kind: Kind,
    tree: TreeId,
    pub(super) id: RunId,
    pub(super) next_seq: u32,
    destination: Destination,
}

impl Run {
    /// Opens a run: durable before any work starts.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record).
    pub(crate) fn open(store: &Store, kind: &Kind, open: Open) -> Result<Self, RecordError> {
        Self::begin(store, kind, open, Destination::Store)
    }

    /// Opens a run whose lines go to the file at `path`, replaced whole on
    /// each write, for another store to record.
    ///
    /// # Errors
    /// As [`Run::open`], and a file that cannot be written.
    pub(crate) fn open_to_file(
        store: &Store,
        kind: &Kind,
        open: Open,
        path: std::path::PathBuf,
    ) -> Result<Self, RecordError> {
        Self::begin(
            store,
            kind,
            open,
            Destination::Returned {
                path,
                lines: Vec::new(),
            },
        )
    }

    fn begin(
        store: &Store,
        kind: &Kind,
        open: Open,
        mut destination: Destination,
    ) -> Result<Self, RecordError> {
        let id = RunId::new();
        let commit = open.commit.clone();
        let line = EventLine {
            v: ENVELOPE_VERSION,
            run: id.clone(),
            seq: 0,
            time: Timestamp::now().into(),
            event: Event::Open(Box::new(open)),
        };
        let first = std::slice::from_ref(&line);
        match &mut destination {
            Destination::Store => {
                record(store, kind, first, &OpensOnly)?;
            }
            Destination::Returned { path, lines } => {
                record_dry_run(store, kind, first, &OpensOnly)?;
                replace_file(path, first)?;
                lines.push(line);
            }
        }
        let tree = store.git().tree_of(commit.object())?;
        Ok(Self {
            store: store.clone(),
            kind: kind.clone(),
            tree: TreeId::parse(tree.as_str()).map_err(git_process::GitError::from)?,
            id,
            next_seq: 1,
            destination,
        })
    }

    /// Finds an unfinished run this producer opened, to continue or close it
    /// after a crash. Nothing is replayed: `declared` and `answered` say where
    /// it stopped.
    ///
    /// # Errors
    /// Refuses a run this store does not hold ([`Refusal::NotRecorded`]), one
    /// that has ended ([`Refusal::Ended`]), and one with missing events
    /// ([`Problem::Gap`]: fetch first).
    pub(crate) fn resume(
        store: &Store,
        kind: &Kind,
        id: &RunId,
        locator: &dyn RunLocator,
    ) -> Result<Resumed, RecordError> {
        let not_recorded = || RecordError::Refused(Refusal::NotRecorded { run: id.clone() });
        let tree = locator.tree_of_run(kind, id)?.ok_or_else(not_recorded)?;
        let note = store.read_note(kind, &tree)?.unwrap_or_default();
        let folded = fold(&note);
        // A gap (another store wrote later events this one lacks) means the
        // next seq is unknown: continuing would collide. Fetch, then resume.
        if let Some(gap) = folded
            .violations
            .iter()
            .find(|v| v.run.as_ref() == Some(id) && matches!(v.problem, Problem::Gap { .. }))
        {
            return Err(RecordError::Refused(Refusal::Rule {
                run: Some(id.clone()),
                problem: gap.problem.clone(),
            }));
        }
        let state = folded
            .runs
            .into_iter()
            .find(|run| &run.id == id)
            .ok_or_else(not_recorded)?;
        if state.end.is_some() {
            return Err(RecordError::Refused(Refusal::Ended { run: id.clone() }));
        }
        Ok(Resumed {
            declared: state.keys.clone(),
            answered: state.results.keys().cloned().collect(),
            run: Self {
                store: store.clone(),
                kind: kind.clone(),
                tree,
                id: id.clone(),
                next_seq: state.last_seq + 1,
                destination: Destination::Store,
            },
        })
    }

    #[must_use]
    pub const fn id(&self) -> &RunId {
        &self.id
    }

    fn line(&mut self, event: Event) -> EventLine {
        let line = EventLine {
            v: ENVELOPE_VERSION,
            run: self.id.clone(),
            seq: self.next_seq,
            time: Timestamp::now().into(),
            event,
        };
        self.next_seq += 1;
        line
    }

    /// Writes `events` as one durable commit; on a refusal nothing is written
    /// and the run's numbering is unchanged.
    pub(super) fn write_all(&mut self, events: Vec<Event>) -> Result<(), RecordError> {
        let first = self.next_seq;
        let lines: Vec<EventLine> = events.into_iter().map(|e| self.line(e)).collect();
        let written = match &self.destination {
            Destination::Store => record(&self.store, &self.kind, &lines, self).map(|_| None),
            Destination::Returned {
                path,
                lines: before,
            } => {
                let whole: Vec<EventLine> = before.iter().chain(&lines).cloned().collect();
                record_dry_run(&self.store, &self.kind, &whole, &OpensOnly)
                    .and_then(|_| replace_file(path, &whole))
                    .map(|()| Some(whole))
            }
        };
        match written {
            Ok(Some(whole)) => {
                if let Destination::Returned { lines, .. } = &mut self.destination {
                    *lines = whole;
                }
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => {
                self.next_seq = first;
                Err(error)
            }
        }
    }

    /// The lines [`Finish::check`] judges: a returned run is checked whole,
    /// since this store holds none of it.
    pub(super) fn checked(&self, mut new: Vec<EventLine>) -> (Vec<EventLine>, &dyn RunLocator) {
        match &self.destination {
            Destination::Store => (new, self),
            Destination::Returned { lines, .. } => {
                let mut whole = lines.clone();
                whole.append(&mut new);
                (whole, &OpensOnly)
            }
        }
    }

    fn write(&mut self, event: Event) -> Result<(), RecordError> {
        self.write_all(vec![event])
    }

    /// Records several keys' outcomes as one durable commit: all or nothing.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record): an undeclared key, a second result or an undeclared
    /// metric anywhere in the batch refuses all of it.
    pub fn results(
        &mut self,
        results: impl IntoIterator<Item = (Key, Outcome)>,
    ) -> Result<(), RecordError> {
        self.write_all(
            results
                .into_iter()
                .map(|(key, outcome)| Event::Result {
                    key,
                    outcome,
                    location: None,
                })
                .collect(),
        )
    }

    /// The rest of the run, written as one durable commit when [`Finish::end`]
    /// is called: a late declaration, the results and the close, all or nothing.
    pub fn finish(self) -> Finish {
        Finish {
            run: self,
            events: Vec::new(),
            conditions: None,
        }
    }

    /// Declares the keys, the metrics learned late and what was built, once the
    /// producer knows them; at most once per run, before any result.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record): keys declared twice, a declaration after a result,
    /// a redefined metric or a derived metric name is refused.
    pub fn declare(
        &mut self,
        keys: Option<Vec<Key>>,
        metrics: BTreeMap<MetricName, Metric>,
        build: Option<Build>,
    ) -> Result<(), RecordError> {
        self.write(Event::Declare {
            keys,
            metrics,
            build,
        })
    }

    /// Records one key's outcome, durable before return.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record): an undeclared key or a second result is refused.
    pub fn result(
        &mut self,
        key: &Key,
        outcome: Outcome,
        location: Option<String>,
    ) -> Result<(), RecordError> {
        self.write(Event::Result {
            key: key.clone(),
            outcome,
            location,
        })
    }

    /// Ends the run.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record): `complete` with keys unanswered is refused.
    pub fn close(
        mut self,
        end: End,
        conditions: Option<serde_json::Value>,
    ) -> Result<(), RecordError> {
        self.write(Event::Close { end, conditions })
    }
}

/// A run found again after a crash, with where it stopped.
#[derive(Debug)]
pub struct Resumed {
    pub run: Run,
    /// The keys it declared; `None` when it stopped before declaring them.
    pub declared: Option<Vec<Key>>,
    /// The keys it answered.
    pub answered: BTreeSet<Key>,
}

impl RunLocator for Run {
    fn tree_of_run(&self, _: &Kind, run: &RunId) -> Result<Option<TreeId>, RecordError> {
        Ok((run == &self.id).then(|| self.tree.clone()))
    }
}
