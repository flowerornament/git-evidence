//! Folding a note into runs.
//!
//! The fold is the one place the run grammar is read. A note is a set of lines:
//! lines are grouped by run and ordered by `seq`, so line order never matters,
//! and an identical line seen twice is one line. A run is read only up to
//! its first missing `seq`, so nothing can be added to a run behind its close. A
//! line or run that breaks the grammar is reported, never silently skipped.
//! A run that closes `complete` must have answered every declared key, or it
//! is not read at all.
//!
//! A run's metrics are declared once each: a measure of an
//! undeclared metric is not read, and a run that redefines a metric or declares
//! a derived one is not read at all, since what its numbers mean is unknown.

use crate::format::event::{
    Build, DERIVED_PREFIX, ENVELOPE_VERSION, End, Event, EventLine, Metric, Open, Outcome,
    Timestamp,
};
use crate::format::id::{IdError, Key, Machine, MetricName, RunId};
use std::collections::BTreeMap;

/// One answered key.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub outcome: Outcome,
    pub location: Option<String>,
}

/// The state of one run after folding its lines.
#[derive(Debug, Clone, PartialEq)]
pub struct RunState {
    pub id: RunId,
    pub open: Open,
    /// The declared keys: from `open`, or from `declare` once known.
    pub keys: Option<Vec<Key>>,
    /// What was built: `open`'s build, completed by `declare`.
    pub build: Build,
    /// The declared metrics: `open.suite.metrics`, extended by `declare`.
    pub metrics: BTreeMap<MetricName, Metric>,
    /// Whether the run's one `declare` has been folded.
    declared: bool,
    /// Whether a `result` line has been folded (read or refused by a rule;
    /// a line skipped for its time is not folded): a `declare` may come only
    /// before every result.
    resulted: bool,
    pub results: BTreeMap<Key, Answered>,
    pub end: Option<End>,
    /// The close's conditions, without interpretation by the reader.
    pub conditions: Option<serde_json::Value>,
    /// The open and close events' times.
    pub opened: Timestamp,
    pub closed: Option<Timestamp>,
    /// The highest `seq` folded: the next event of this run takes `last_seq + 1`.
    pub last_seq: u32,
}

impl RunState {
    /// The machine the run's open names; `None` when it names none (the
    /// envelope spells that `""`).
    ///
    /// # Errors
    /// The name is not a machine name ([`Machine::parse`]).
    pub fn machine(&self) -> Result<Option<Machine>, IdError> {
        match self.open.environment.machine.as_str() {
            "" => Ok(None),
            name => Machine::parse(name).map(Some),
        }
    }

    /// No `end` folded yet: the run may still be written to.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.end.is_none()
    }

    /// The outcome for `key`: its result; else, once the run ended, the end's
    /// refusal. `None` while the run is open, or for a key it never declared.
    #[must_use]
    pub fn outcome(&self, key: &Key) -> Option<Outcome> {
        if let Some(answered) = self.results.get(key) {
            return Some(answered.outcome.clone());
        }
        let declared = self.keys.as_ref().is_some_and(|keys| keys.contains(key));
        match &self.end {
            Some(End::Refused(reason)) if declared => Some(Outcome::Refused(reason.clone())),
            // A folded `complete` run answered every declared key.
            _ => None,
        }
    }
}

/// A line or run that broke the grammar or could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Violation {
    pub run: Option<RunId>,
    pub problem: Problem,
}

/// The rule a line or run broke: the run grammar or the metric
/// declarations. One name per rule, so a reader and a
/// refusal say which rule, never only prose.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Problem {
    #[error("unreadable event: {0}")]
    Unreadable(String),
    #[error("two different events claim seq {seq}")]
    ConflictingSeq { seq: u32 },
    #[error("a run with no events")]
    Empty,
    #[error("seq {seq}: envelope version {found}, not {ENVELOPE_VERSION}")]
    Envelope { seq: u32, found: u32 },
    #[error("a run whose first event is missing")]
    FirstMissing,
    #[error("a run whose first event is not open")]
    FirstNotOpen,
    #[error("seq {seq}: a time that is not YYYY-MM-DDTHH:MM:SS.mmmZ")]
    Time { seq: u32 },
    #[error("seq {seq}: events {from}–{to} are missing; the run is read up to seq {read_to}")]
    Gap {
        seq: u32,
        from: u32,
        to: u32,
        read_to: u32,
    },
    #[error("seq {seq}: an event after close")]
    AfterClose { seq: u32 },
    #[error("seq {seq}: a second open")]
    SecondOpen { seq: u32 },
    #[error("seq {seq}: a second declare")]
    SecondDeclare { seq: u32 },
    #[error("seq {seq}: a declare after a result")]
    DeclareAfterResult { seq: u32 },
    #[error("seq {seq}: keys declared twice")]
    KeysTwice { seq: u32 },
    #[error("seq {seq}: {key} measures {metric}, which the run did not declare")]
    UndeclaredMetric {
        seq: u32,
        key: Key,
        metric: MetricName,
    },
    #[error("seq {seq}: a second result for one key")]
    SecondResult { seq: u32 },
    #[error("seq {seq}: a result for an undeclared key")]
    UndeclaredKey { seq: u32 },
    #[error("seq {seq}: closed complete with {unanswered} declared keys unanswered")]
    IncompleteClose { seq: u32, unanswered: usize },
    #[error(
        "seq {seq}: {metric} is a derived metric (`{DERIVED_PREFIX}…`); a run may not declare it"
    )]
    ReservedMetric { seq: u32, metric: MetricName },
    #[error("seq {seq}: {metric} redefined: {} {:?} was declared, then {} {:?}", was.unit, was.better, now.unit, now.better)]
    MetricRedefined {
        seq: u32,
        metric: MetricName,
        was: Metric,
        now: Metric,
    },
    #[error("seq {seq}: {metric} is declared without a unit")]
    NoUnit { seq: u32, metric: MetricName },
    #[error("seq 0: {0}")]
    Machine(IdError),
}

/// Runs and grammar violations in one note.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Folded {
    pub runs: Vec<RunState>,
    pub violations: Vec<Violation>,
}

/// Parses a note's lines, keeping each distinct line once, grouped by run and seq.
fn group(
    note: &[u8],
    violations: &mut Vec<Violation>,
) -> BTreeMap<RunId, BTreeMap<u32, Vec<EventLine>>> {
    let mut by_run: BTreeMap<RunId, BTreeMap<u32, Vec<EventLine>>> = BTreeMap::new();
    for raw in note.split(|b| *b == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<EventLine>(raw) {
            Ok(line) => {
                let slot = by_run
                    .entry(line.run.clone())
                    .or_default()
                    .entry(line.seq)
                    .or_default();
                if !slot.contains(&line) {
                    slot.push(line);
                }
            }
            Err(error) => violations.push(Violation {
                run: None,
                problem: Problem::Unreadable(error.to_string()),
            }),
        }
    }
    by_run
}

/// Folds a note's lines, in any order.
#[must_use]
pub fn fold(note: &[u8]) -> Folded {
    let mut violations = Vec::new();
    let by_run = group(note, &mut violations);
    let mut runs = Vec::new();
    for (id, lines) in by_run {
        match fold_run(&id, lines) {
            Ok((run, problems)) => {
                // A machine named, but not as a machine name: what the run's
                // values are about cannot be told, so it is not read.
                if let Err(error) = run.machine() {
                    violations.push(Violation {
                        run: Some(id),
                        problem: Problem::Machine(error),
                    });
                    continue;
                }
                violations.extend(problems.into_iter().map(|problem| Violation {
                    run: Some(id.clone()),
                    problem,
                }));
                runs.push(run);
            }
            Err(problem) => violations.push(Violation {
                run: Some(id),
                problem,
            }),
        }
    }
    Folded { runs, violations }
}

/// One run's lines in `seq` order. A run that cannot be folded at all is an
/// error; a line that breaks the grammar is a problem reported beside the run.
fn fold_run(
    id: &RunId,
    lines: BTreeMap<u32, Vec<EventLine>>,
) -> Result<(RunState, Vec<Problem>), Problem> {
    let mut events = in_seq_order(lines)?.into_iter();
    let first = events.next().ok_or(Problem::Empty)?;
    let mut run = RunState::opened(id, first)?;
    let mut problems = Vec::new();
    for line in events {
        // A run is read only as far as its events are contiguous: after a gap,
        // nothing can be told apart from a line that never arrived.
        if line.seq != run.last_seq + 1 {
            problems.push(Problem::Gap {
                seq: line.seq,
                from: run.last_seq + 1,
                to: line.seq - 1,
                read_to: run.last_seq,
            });
            break;
        }
        run.last_seq = line.seq;
        problems.extend(run.apply(line)?);
    }
    Ok((run, problems))
}

/// One line per `seq`, in order, all of this envelope version: two
/// different lines claiming one `seq` leave the run unreadable, and a line
/// of another version means something else, so reading it as this one would
/// misread it.
fn in_seq_order(lines: BTreeMap<u32, Vec<EventLine>>) -> Result<Vec<EventLine>, Problem> {
    let ordered = lines
        .into_iter()
        .map(|(seq, claims)| {
            let [line] =
                <[EventLine; 1]>::try_from(claims).map_err(|_| Problem::ConflictingSeq { seq })?;
            Ok(line)
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    if let Some(line) = ordered.iter().find(|line| line.v != ENVELOPE_VERSION) {
        return Err(Problem::Envelope {
            seq: line.seq,
            found: line.v,
        });
    }
    Ok(ordered)
}

impl RunState {
    /// The run as its `open` (seq 0) starts it.
    fn opened(id: &RunId, first: EventLine) -> Result<Self, Problem> {
        if first.seq != 0 {
            return Err(Problem::FirstMissing);
        }
        let Event::Open(open) = first.event else {
            return Err(Problem::FirstNotOpen);
        };
        let Ok(opened) = Timestamp::parse(&first.time) else {
            return Err(Problem::Time { seq: 0 });
        };
        let mut metrics = BTreeMap::new();
        declare_metrics(&mut metrics, &open.suite.metrics, 0)?;
        Ok(Self {
            id: id.clone(),
            keys: open.keys.clone(),
            build: open.build.clone(),
            metrics,
            open: *open,
            declared: false,
            resulted: false,
            results: BTreeMap::new(),
            end: None,
            conditions: None,
            opened,
            closed: None,
            last_seq: 0,
        })
    }

    /// Folds the next line in: a problem it breaks is returned beside the
    /// run, and one that leaves the run unreadable is the error.
    fn apply(&mut self, line: EventLine) -> Result<Option<Problem>, Problem> {
        let seq = line.seq;
        let Ok(time) = Timestamp::parse(&line.time) else {
            return Ok(Some(Problem::Time { seq }));
        };
        if self.end.is_some() {
            return Ok(Some(Problem::AfterClose { seq }));
        }
        match line.event {
            Event::Open(_) => Ok(Some(Problem::SecondOpen { seq })),
            Event::Declare {
                keys,
                metrics,
                build,
            } => self.declare(seq, keys, &metrics, build),
            Event::Result {
                key,
                outcome,
                location,
            } => {
                self.resulted = true;
                Ok(self.answer(seq, key, Answered { outcome, location }))
            }
            Event::Close { end, conditions } => {
                self.close(seq, end, conditions, time)?;
                Ok(None)
            }
        }
    }

    /// The run's one `declare`: keys not named at open, metrics learned
    /// late, and what was built. One after a result leaves the run
    /// unreadable: it could give an earlier line a meaning it did not have
    /// when it was written.
    fn declare(
        &mut self,
        seq: u32,
        keys: Option<Vec<Key>>,
        metrics: &BTreeMap<MetricName, Metric>,
        build: Option<Build>,
    ) -> Result<Option<Problem>, Problem> {
        if self.resulted {
            return Err(Problem::DeclareAfterResult { seq });
        }
        if self.declared {
            return Ok(Some(Problem::SecondDeclare { seq }));
        }
        self.declared = true;
        if let Some(keys) = keys {
            if self.keys.is_some() {
                return Ok(Some(Problem::KeysTwice { seq }));
            }
            self.keys = Some(keys);
        }
        declare_metrics(&mut self.metrics, metrics, seq)?;
        if let Some(build) = build {
            merge_build(&mut self.build, build);
        }
        Ok(None)
    }

    /// A key's one result: of a declared key, measuring only declared
    /// metrics.
    fn answer(&mut self, seq: u32, key: Key, answered: Answered) -> Option<Problem> {
        if let Outcome::Measured(measures) = &answered.outcome
            && let Some(m) = measures
                .iter()
                .find(|m| !self.metrics.contains_key(&m.metric))
        {
            return Some(Problem::UndeclaredMetric {
                seq,
                key,
                metric: m.metric.clone(),
            });
        }
        if !self.keys.as_ref().is_some_and(|keys| keys.contains(&key)) {
            return Some(Problem::UndeclaredKey { seq });
        }
        match self.results.entry(key) {
            std::collections::btree_map::Entry::Occupied(_) => Some(Problem::SecondResult { seq }),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(answered);
                None
            }
        }
    }

    /// The close. A run that claims completion it does not have is not read
    /// at all: keeping it would let it stand for keys it never answered.
    fn close(
        &mut self,
        seq: u32,
        end: End,
        conditions: Option<serde_json::Value>,
        time: Timestamp,
    ) -> Result<(), Problem> {
        if end == End::Complete {
            let unanswered = self
                .keys
                .iter()
                .flatten()
                .filter(|key| !self.results.contains_key(*key))
                .count();
            if unanswered > 0 {
                return Err(Problem::IncompleteClose { seq, unanswered });
            }
        }
        self.end = Some(end);
        self.conditions = conditions;
        self.closed = Some(time);
        Ok(())
    }
}

/// Adds `more` to a run's declarations: a metric declared again must mean the
/// same; a redefinition or a derived name leaves the run unreadable.
fn declare_metrics(
    into: &mut BTreeMap<MetricName, Metric>,
    more: &BTreeMap<MetricName, Metric>,
    seq: u32,
) -> Result<(), Problem> {
    for (name, metric) in more {
        if name.as_str().starts_with(DERIVED_PREFIX) {
            return Err(Problem::ReservedMetric {
                seq,
                metric: name.clone(),
            });
        }
        match into.get(name) {
            Some(earlier) if earlier != metric => {
                return Err(Problem::MetricRedefined {
                    seq,
                    metric: name.clone(),
                    was: earlier.clone(),
                    now: metric.clone(),
                });
            }
            Some(_) => {}
            None => {
                if metric.unit.trim().is_empty() {
                    return Err(Problem::NoUnit {
                        seq,
                        metric: name.clone(),
                    });
                }
                into.insert(name.clone(), metric.clone());
            }
        }
    }
    Ok(())
}

fn merge_build(into: &mut Build, from: Build) {
    if from.profile.is_some() {
        into.profile = from.profile;
    }
    if from.toolchain.is_some() {
        into.toolchain = from.toolchain;
    }
    if from.artifact.is_some() {
        into.artifact = from.artifact;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::event::Reason;
    use crate::format::event::{ENVELOPE_VERSION, Metric};
    use crate::format::id::{CommitId, MetricName, ProducerName};

    pub(crate) const RUN: &str = "01JA8Z3K4M0000000000000000";

    pub(crate) fn key(name: &str) -> Key {
        Key::parse(name).expect("key")
    }

    pub(crate) fn line(seq: u32, event: Event) -> String {
        serde_json::to_string(&EventLine {
            v: ENVELOPE_VERSION,
            run: RunId::parse(RUN).expect("run"),
            seq,
            time: format!("2026-10-08T00:00:0{seq}.000Z"),
            event,
        })
        .expect("json")
    }

    pub(crate) fn open(keys: Option<&[&str]>) -> Event {
        let open = Open::new(
            CommitId::parse(&"c".repeat(40)).expect("commit"),
            ProducerName::parse("bench-suite").expect("producer"),
        )
        .metric(p99_ns(), ns());
        Event::Open(Box::new(match keys {
            Some(keys) => open.keys(keys.iter().map(|k| key(k))),
            None => open,
        }))
    }

    pub(crate) fn p99_ns() -> MetricName {
        MetricName::parse("block.p99_ns").expect("metric")
    }

    pub(crate) fn ns() -> Metric {
        Metric::lower("ns")
    }

    pub(crate) fn measured() -> Outcome {
        Outcome::measured(p99_ns(), 1.0).expect("finite")
    }

    fn declare(metrics: &[(&str, Metric)]) -> Event {
        Event::Declare {
            keys: None,
            metrics: metrics
                .iter()
                .map(|(name, m)| (MetricName::parse(name).expect("metric"), m.clone()))
                .collect(),
            build: None,
        }
    }

    /// A run on a machine of `name` (the envelope's spelling).
    fn on_machine(name: &str) -> Event {
        let Event::Open(mut open) = open(Some(&["a"])) else {
            unreachable!("open() is an open")
        };
        open.environment.machine = name.to_owned();
        Event::Open(open)
    }

    #[test]
    fn a_run_names_a_machine_or_none_and_anything_else_is_not_read() {
        let named = fold(&note(&[line(0, on_machine("bench"))]));
        assert_eq!(
            named.runs[0].machine(),
            Ok(Some(Machine::parse("bench").expect("machine")))
        );
        let unnamed = fold(&note(&[line(0, on_machine(""))]));
        assert_eq!(unnamed.runs[0].machine(), Ok(None));
        let malformed = fold(&note(&[line(0, on_machine("two words"))]));
        assert!(malformed.runs.is_empty(), "{malformed:?}");
        assert!(matches!(
            malformed.violations[0].problem,
            Problem::Machine(_)
        ));
    }

    #[test]
    fn a_metric_is_declared_once_and_never_redefined() {
        // Repeating a declaration is harmless.
        let same = fold(&note(&[
            line(0, open(Some(&["a"]))),
            line(1, declare(&[("block.p99_ns", ns())])),
            result(2, "a", measured()),
        ]));
        assert!(same.violations.is_empty(), "{:?}", same.violations);
        assert_eq!(same.runs[0].metrics[&p99_ns()], ns());
        // A different unit makes the run's numbers ambiguous: it is not read.
        let us = Metric {
            unit: "us".into(),
            ..ns()
        };
        let redefined = fold(&note(&[
            line(0, open(Some(&["a"]))),
            line(1, declare(&[("block.p99_ns", us)])),
        ]));
        assert!(redefined.runs.is_empty());
        assert!(matches!(
            redefined.violations[0].problem,
            Problem::MetricRedefined { seq: 1, .. }
        ));
        // A derived name cannot be declared.
        let derived = fold(&note(&[
            line(0, open(Some(&["a"]))),
            line(1, declare(&[("verdict.pass.duration_ms", ns())])),
        ]));
        assert!(derived.runs.is_empty());
        assert!(matches!(
            derived.violations[0].problem,
            Problem::ReservedMetric { seq: 1, .. }
        ));
    }

    #[test]
    fn a_measure_of_an_undeclared_metric_is_not_read() {
        let undeclared = Outcome::measured(MetricName::parse("block.p50_ns").expect("metric"), 1.0)
            .expect("finite");
        let folded = fold(&note(&[
            line(0, open(Some(&["a"]))),
            result(1, "a", undeclared),
        ]));
        assert!(
            folded.runs[0].results.is_empty(),
            "the key stays unanswered"
        );
        assert!(matches!(
            folded.violations[0].problem,
            Problem::UndeclaredMetric { seq: 1, .. }
        ));
    }

    fn result(seq: u32, name: &str, outcome: Outcome) -> String {
        line(
            seq,
            Event::Result {
                key: key(name),
                outcome,
                location: None,
            },
        )
    }

    fn note(lines: &[String]) -> Vec<u8> {
        let mut text = lines.join("\n");
        text.push('\n');
        text.into_bytes()
    }

    #[test]
    fn line_order_does_not_matter_and_repeats_are_one_line() {
        let lines = [
            line(0, open(Some(&["a", "b"]))),
            result(1, "a", measured()),
            line(
                2,
                Event::Close {
                    end: End::Refused(Reason::new("interrupted")),
                    conditions: None,
                },
            ),
        ];
        let forward = fold(&note(&lines));
        let mut shuffled = vec![
            lines[2].clone(),
            lines[0].clone(),
            lines[1].clone(),
            lines[1].clone(),
        ];
        shuffled.sort();
        assert_eq!(fold(&note(&shuffled)), forward, "order-free");
        let run = &forward.runs[0];
        assert_eq!(run.outcome(&key("a")), Some(measured()));
        assert_eq!(
            run.outcome(&key("b")),
            Some(Outcome::Refused(Reason::new("interrupted"))),
            "an ended run assigns every declared key"
        );
    }

    #[test]
    fn keys_can_be_declared_after_the_build() {
        let folded = fold(&note(&[
            line(0, open(None)),
            line(
                1,
                Event::Declare {
                    keys: Some(vec![key("a")]),
                    metrics: BTreeMap::new(),
                    build: None,
                },
            ),
            result(2, "a", measured()),
        ]));
        assert!(folded.violations.is_empty(), "{:?}", folded.violations);
        assert!(folded.runs[0].is_open());
        assert_eq!(folded.runs[0].outcome(&key("a")), Some(measured()));
        assert_eq!(folded.runs[0].last_seq, 2);
    }

    #[test]
    fn grammar_breaks_are_reported() {
        let folded = fold(&note(&[
            line(0, open(Some(&["a"]))),
            result(1, "zzz", measured()),
            result(2, "a", measured()),
            result(3, "a", measured()),
            line(
                4,
                Event::Close {
                    end: End::Complete,
                    conditions: None,
                },
            ),
            result(5, "a", measured()),
            "not json".into(),
        ]));
        let problems: Vec<&Problem> = folded.violations.iter().map(|v| &v.problem).collect();
        assert!(problems.iter().any(|p| matches!(p, Problem::Unreadable(_))));
        assert!(problems.contains(&&Problem::UndeclaredKey { seq: 1 }));
        assert!(problems.contains(&&Problem::SecondResult { seq: 3 }));
        assert!(problems.contains(&&Problem::AfterClose { seq: 5 }));
    }

    #[test]
    fn a_run_that_declares_after_a_result_is_not_read() {
        // A late declaration could give an earlier line a meaning it did not
        // have when written; the grammar puts declare before every result.
        let folded = fold(&note(&[
            line(0, open(Some(&["a", "b"]))),
            result(1, "a", measured()),
            line(2, declare(&[("rss_bytes", ns())])),
            result(3, "b", measured()),
        ]));
        assert!(folded.runs.is_empty(), "{:?}", folded.runs);
        assert_eq!(
            folded.violations[0].problem,
            Problem::DeclareAfterResult { seq: 2 }
        );
    }

    #[test]
    fn a_declare_after_any_result_line_leaves_the_run_unreadable_in_any_order() {
        let undeclared_key = [
            line(0, open(Some(&["a"]))),
            result(1, "zzz", measured()),
            line(2, declare(&[])),
        ];
        let second_declare = [
            line(0, open(Some(&["a", "b"]))),
            line(1, declare(&[])),
            result(2, "a", measured()),
            line(3, declare(&[])),
        ];
        for (lines, seq) in [(&undeclared_key[..], 2), (&second_declare[..], 3)] {
            let mut shuffled = lines.to_vec();
            shuffled.reverse();
            shuffled.push(lines[1].clone());
            for note_lines in [lines.to_vec(), shuffled] {
                let folded = fold(&note(&note_lines));
                assert!(folded.runs.is_empty(), "{:?}", folded.runs);
                assert!(
                    folded.violations.contains(&Violation {
                        run: Some(RunId::parse(RUN).expect("run")),
                        problem: Problem::DeclareAfterResult { seq },
                    }),
                    "{:?}",
                    folded.violations
                );
            }
        }
        // A declare before any result is read.
        let read = fold(&note(&[
            line(0, open(None)),
            line(
                1,
                Event::Declare {
                    keys: Some(vec![key("a")]),
                    metrics: BTreeMap::new(),
                    build: None,
                },
            ),
            line(
                2,
                Event::Close {
                    end: End::Refused(Reason::new("interrupted")),
                    conditions: None,
                },
            ),
        ]));
        assert!(read.violations.is_empty(), "{:?}", read.violations);
        assert_eq!(read.runs.len(), 1);
    }

    #[test]
    fn a_line_skipped_for_its_time_is_not_otherwise_read() {
        // A result with a malformed time is skipped and is no result for the
        // declare rule, so a declare after it is read.
        let mut bad_time = result(1, "a", measured());
        bad_time = bad_time.replace("2026-10-08T00:00:01.000Z", "2026-10-08 00:00:01");
        let folded = fold(&note(&[
            line(0, open(Some(&["a"]))),
            bad_time,
            line(2, declare(&[("rss_bytes", ns())])),
        ]));
        assert_eq!(folded.runs.len(), 1, "{:?}", folded.violations);
        assert_eq!(
            folded
                .violations
                .iter()
                .map(|v| &v.problem)
                .collect::<Vec<_>>(),
            [&Problem::Time { seq: 1 }]
        );
        assert!(
            folded.runs[0]
                .metrics
                .contains_key(&MetricName::parse("rss_bytes").expect("metric"))
        );
    }

    #[test]
    fn a_run_closed_complete_with_keys_unanswered_is_not_read() {
        let folded = fold(&note(&[
            line(0, open(Some(&["a", "b"]))),
            result(1, "a", measured()),
            line(
                2,
                Event::Close {
                    end: End::Complete,
                    conditions: None,
                },
            ),
        ]));
        assert!(folded.runs.is_empty(), "it can settle nothing");
        assert_eq!(
            folded.violations[0].problem,
            Problem::IncompleteClose {
                seq: 2,
                unanswered: 1
            }
        );
    }

    #[test]
    fn two_different_events_at_one_seq_refuse_the_run() {
        let folded = fold(&note(&[
            line(0, open(Some(&["a"]))),
            result(1, "a", measured()),
            result(1, "a", Outcome::Refused(Reason::new("noisy_host"))),
        ]));
        assert!(folded.runs.is_empty());
        assert_eq!(
            folded.violations[0].problem,
            Problem::ConflictingSeq { seq: 1 }
        );
    }
}
