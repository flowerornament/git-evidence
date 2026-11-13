//! The rest of a run, gathered and written as one all-or-nothing commit.

use super::batch::record_dry_run;
use super::{RecordError, Recorded, Run};
use crate::format::event::{
    Declaration, ENVELOPE_VERSION, End, Event, EventLine, Outcome, Timestamp,
};
use crate::format::id::Key;

/// The remaining events of a run, gathered for one all-or-nothing commit.
#[derive(Debug)]
#[must_use = "nothing is written until `end` is called"]
pub struct Finish {
    pub(super) run: Run,
    pub(super) events: Vec<Event>,
    pub(super) conditions: Option<serde_json::Value>,
}

impl Finish {
    /// The run's one late declaration: keys, metrics and what was built.
    /// It must come before any result of the run, streamed or finished; a
    /// declaration after a result is refused when the run is written.
    pub fn declare(mut self, declaration: Declaration) -> Self {
        self.events.push(declaration.into());
        self
    }

    /// Outcomes, each one key's.
    pub fn results(mut self, results: impl IntoIterator<Item = (Key, Outcome)>) -> Self {
        self.events
            .extend(results.into_iter().map(|(key, outcome)| Event::Result {
                key,
                outcome,
                location: None,
            }));
        self
    }

    /// One outcome with the code site that produced it.
    pub fn result_at(mut self, key: Key, outcome: Outcome, location: impl Into<String>) -> Self {
        self.events.push(Event::result_at(key, outcome, location));
        self
    }

    /// Readings taken while the run executed, a JSON object: provenance only.
    pub fn conditions(mut self, conditions: serde_json::Value) -> Self {
        self.conditions = Some(conditions);
        self
    }

    /// What [`Finish::end`] would write with `end`, without writing: every
    /// refusal it would give, and the run's size after it, to compare with
    /// the kind's budget before trimming.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record).
    pub fn check(&self, end: &End) -> Result<Recorded, RecordError> {
        let mut seq = self.run.next_seq;
        let mut events = self.events.clone();
        events.push(Event::Close {
            end: end.clone(),
            conditions: self.conditions.clone(),
        });
        let lines: Vec<EventLine> = events
            .into_iter()
            .map(|event| {
                let line = EventLine {
                    v: ENVELOPE_VERSION,
                    run: self.run.id.clone(),
                    seq,
                    time: Timestamp::now().into(),
                    event,
                };
                seq += 1;
                line
            })
            .collect();
        let (lines, locator) = self.run.checked(lines);
        record_dry_run(&self.run.store, &self.run.kind, &lines, locator)
    }

    /// Writes everything gathered, then the close, as one commit.
    ///
    /// # Errors
    /// As [`Evidence::record`](crate::Evidence::record): any line breaking a rule refuses the whole commit.
    pub fn end(self, end: End) -> Result<(), RecordError> {
        let Self {
            mut run,
            mut events,
            conditions,
        } = self;
        events.push(Event::Close { end, conditions });
        run.write_all(events)
    }
}
