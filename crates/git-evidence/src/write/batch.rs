//! Recording a batch: any set of lines for one kind, a whole run or the next
//! events of a run being streamed. Each run's tree comes from its `open` (the
//! commit's tree) or, for a run already recorded, from wherever it was
//! recorded. The whole batch is checked before anything is written; lines
//! already present are ignored, so a batch delivered twice changes nothing;
//! each tree's new lines are appended in one commit, on disk before return.

use super::lines::check_lines;
use super::{RecordError, Refusal};
use crate::format::event::{Event, EventLine};
use crate::format::fold::fold;
use crate::format::id::{Kind, RunId, TreeId};
use crate::rows::{TidyRows, cell, columns};
use crate::store::Store;
use std::collections::{BTreeMap, BTreeSet};

/// What a record did.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Recorded {
    /// Lines appended (or, from [`Evidence::record_dry_run`](crate::Evidence::record_dry_run), that would be).
    pub written: usize,
    /// Lines that were already recorded and were ignored.
    pub already_present: usize,
    /// Bytes the batch adds to the notes, encoded lines with their newlines.
    pub bytes_added: u64,
    /// Each run in the batch: its lines, and its size once the batch is in.
    pub runs: BTreeMap<RunId, RunLines>,
}

/// One run's lines in a batch: written, already recorded, and the run's
/// encoded size once the batch is in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RunLines {
    pub written: usize,
    pub already_present: usize,
    pub run_bytes: u64,
}

impl Recorded {
    /// `run`'s encoded size once the batch is in; `None` for a run not in
    /// the batch.
    #[must_use]
    pub fn run_bytes(&self, run: &RunId) -> Option<u64> {
        self.runs.get(run).map(|lines| lines.run_bytes)
    }

    /// Counts `batch` against the lines of its note already there: each
    /// run's lines written and already present, and its size once written.
    /// Returns the lines to write, encoded.
    fn tally(&mut self, note: &[u8], batch: &[&EventLine]) -> Result<Vec<Vec<u8>>, RecordError> {
        let present: Vec<EventLine> = note
            .split(|b| *b == b'\n')
            .filter_map(|raw| serde_json::from_slice(raw).ok())
            .collect();
        let mut fresh = Vec::new();
        for line in batch {
            let lines = self
                .runs
                .entry(line.run.clone())
                .or_insert_with(|| RunLines {
                    run_bytes: present
                        .iter()
                        .filter(|p| p.run == line.run)
                        .filter_map(|p| serde_json::to_vec(p).ok())
                        .map(|e| e.len() as u64 + 1)
                        .sum(),
                    ..RunLines::default()
                });
            if present.contains(*line) {
                self.already_present += 1;
                lines.already_present += 1;
            } else {
                let encoded = serde_json::to_vec(line)?;
                let size = encoded.len() as u64 + 1;
                self.bytes_added += size;
                lines.run_bytes += size;
                lines.written += 1;
                fresh.push(encoded);
            }
        }
        Ok(fresh)
    }
}

/// Where a run already recorded lives: its tree, if it is recorded at all.
pub(crate) trait RunLocator {
    /// # Errors
    /// Fails when the lookup itself fails.
    fn tree_of_run(&self, kind: &Kind, run: &RunId) -> Result<Option<TreeId>, RecordError>;
}

/// No run is recorded yet: every batch must carry its runs' `open`.
pub(crate) struct OpensOnly;

impl RunLocator for OpensOnly {
    fn tree_of_run(&self, _: &Kind, _: &RunId) -> Result<Option<TreeId>, RecordError> {
        Ok(None)
    }
}

/// Records `lines` of `kind`: checked whole, then each tree's new lines
/// appended under the kind's lock, on disk before return.
///
/// # Errors
/// Refuses lines in the wrong format, a run whose tree cannot be found, and a
/// batch that would break the run grammar; fails when Git or the disk does.
pub(crate) fn record(
    store: &Store,
    kind: &Kind,
    lines: &[EventLine],
    locator: &dyn RunLocator,
) -> Result<Recorded, RecordError> {
    check_lines(lines)?;
    let trees = trees_of(store, kind, lines, locator)?;
    let lock = store.lock(&Store::notes_ref(kind))?;
    let batch = plan(store, kind, trees)?;
    crate::write::fault::point("record:checked");
    if !batch.appends.is_empty() {
        store.append(&lock, kind, &batch.appends)?;
    }
    Ok(batch.recorded)
}

/// What [`record`] would do with `lines`, without writing: every refusal it
/// would give, the bytes the batch would add, and each run's size after it.
/// A producer checks a final batch against its budget before sending it.
///
/// # Errors
/// As [`Evidence::record`](crate::Evidence::record).
pub(crate) fn record_dry_run(
    store: &Store,
    kind: &Kind,
    lines: &[EventLine],
    locator: &dyn RunLocator,
) -> Result<Recorded, RecordError> {
    check_lines(lines)?;
    let trees = trees_of(store, kind, lines, locator)?;
    Ok(plan(store, kind, trees)?.recorded)
}

/// The batch's lines by the tree each run is about.
fn trees_of<'a>(
    store: &Store,
    kind: &Kind,
    lines: &'a [EventLine],
    locator: &dyn RunLocator,
) -> Result<BTreeMap<TreeId, Vec<&'a EventLine>>, RecordError> {
    let mut trees: BTreeMap<TreeId, Vec<&EventLine>> = BTreeMap::new();
    let runs: BTreeSet<&RunId> = lines.iter().map(|line| &line.run).collect();
    for run in runs {
        let tree = tree_of_run(store, kind, lines, run, locator)?;
        trees
            .entry(tree)
            .or_default()
            .extend(lines.iter().filter(|line| &line.run == run));
    }
    Ok(trees)
}

/// A run's tree: its open's commit's tree when the batch opens it, else
/// where it is already recorded.
fn tree_of_run(
    store: &Store,
    kind: &Kind,
    lines: &[EventLine],
    run: &RunId,
    locator: &dyn RunLocator,
) -> Result<TreeId, RecordError> {
    let open = lines.iter().find_map(|line| match &line.event {
        Event::Open(open) if &line.run == run => Some(open),
        _ => None,
    });
    let Some(open) = open else {
        return locator
            .tree_of_run(kind, run)?
            .ok_or_else(|| Refusal::NoOpen { run: run.clone() }.into());
    };
    // One question: the commit's tree, refused unless it names a commit.
    let Some(tree) = store
        .git()
        .resolve(&format!("{}^{{commit}}^{{tree}}", open.commit))?
    else {
        return Err(Refusal::UnknownCommit {
            run: run.clone(),
            commit: open.commit.clone(),
        }
        .into());
    };
    let tree = TreeId::parse(tree.as_str()).map_err(git_process::GitError::from)?;
    // A run is about one tree; a second open elsewhere would make every copy
    // unreadable.
    if let Some(recorded) = locator.tree_of_run(kind, run)?
        && recorded != tree
    {
        return Err(Refusal::OtherTree {
            run: run.clone(),
            recorded,
        }
        .into());
    }
    Ok(tree)
}

/// A batch once every tree is checked: what to append, and what recording
/// it does.
struct Batch {
    /// Each tree's new lines, encoded.
    appends: Vec<(TreeId, Vec<Vec<u8>>)>,
    recorded: Recorded,
}

/// Checks every tree of the batch before any is written, so a refused batch
/// writes nothing.
fn plan(
    store: &Store,
    kind: &Kind,
    trees: BTreeMap<TreeId, Vec<&EventLine>>,
) -> Result<Batch, RecordError> {
    let budget = store.budget(kind)?;
    let mut appends = Vec::new();
    let mut report = Recorded::default();
    for (tree, batch) in trees {
        let existing = store.read_note(kind, &tree)?.unwrap_or_default();
        let fresh = report.tally(&existing, &batch)?;
        if fresh.is_empty() {
            continue;
        }
        refuse_new_violations(&existing, &fresh)?;
        report.written += fresh.len();
        appends.push((tree, fresh));
    }
    if let Some(budget) = budget {
        refuse_over_budget(kind, budget, &report)?;
    }
    Ok(Batch {
        appends,
        recorded: report,
    })
}

/// Refuses only what this batch breaks: a note can already hold another
/// store's violations, which are not this writer's to answer for.
fn refuse_new_violations(existing: &[u8], fresh: &[Vec<u8>]) -> Result<(), Refusal> {
    let mut combined = existing.to_vec();
    for line in fresh {
        combined.extend_from_slice(line);
        combined.push(b'\n');
    }
    let before = fold(existing).violations;
    match fold(&combined)
        .violations
        .into_iter()
        .find(|violation| !before.contains(violation))
    {
        Some(broken) => Err(Refusal::Rule {
            run: broken.run,
            problem: broken.problem,
        }),
        None => Ok(()),
    }
}

/// A run's size is bounded per kind: evidence is permanent in every clone,
/// so a run this batch writes to and takes over the budget is refused, not
/// trimmed.
fn refuse_over_budget(kind: &Kind, budget: u64, report: &Recorded) -> Result<(), Refusal> {
    match report
        .runs
        .iter()
        .find(|(_, lines)| lines.written > 0 && lines.run_bytes > budget)
    {
        Some((run, lines)) => Err(Refusal::OverBudget {
            run: run.clone(),
            kind: kind.clone(),
            budget,
            bytes: lines.run_bytes,
        }),
        None => Ok(()),
    }
}

/// What `record` did: one row per run in the batch, its lines written and
/// already recorded, and its size once the batch is in.
impl TidyRows for Recorded {
    fn header(&self) -> Vec<String> {
        columns(&["run", "written", "already_present", "run_bytes"])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.runs
            .iter()
            .map(|(run, lines)| {
                vec![
                    cell(run),
                    cell(lines.written),
                    cell(lines.already_present),
                    cell(lines.run_bytes),
                ]
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::event::{Build, ENVELOPE_VERSION, End, Metric, Open, Outcome, Try};
    use crate::format::event::{Environment, Producer, Reason, Suite};
    use crate::format::fold::Problem;
    use crate::format::fold::tests::measured;
    use crate::format::id::{CommitId, ProducerName};
    use crate::format::id::{Key, MetricName};
    use crate::write::Run;
    use git_process::testing;

    fn setup() -> (tempfile::TempDir, Store, Kind, Open, TreeId) {
        let (dir, git) = testing::repo();
        let master = git
            .resolve("refs/heads/master")
            .expect("ok")
            .expect("master");
        let store = Store::open(git.git_dir());
        let tree = TreeId::parse(git.tree_of(&master).expect("tree").as_str()).expect("tree");
        let open = Open {
            commit: CommitId::parse(master.as_str()).expect("commit"),
            producer: Producer {
                name: ProducerName::parse("bench-suite").expect("producer"),
                version: None,
            },
            suite: Suite {
                metrics: [(
                    crate::format::fold::tests::p99_ns(),
                    crate::format::fold::tests::ns(),
                )]
                .into(),
                ..Suite::default()
            },
            environment: Environment {
                machine: "bench".into(),
                ..Environment::default()
            },
            build: Build::default(),
            keys: Some(vec![key("a"), key("b")]),
            source: None,
        };
        (dir, store, Kind::parse("perf").expect("kind"), open, tree)
    }

    fn key(name: &str) -> Key {
        Key::parse(name).expect("key")
    }

    fn line(run: &RunId, seq: u32, event: Event) -> EventLine {
        EventLine {
            v: ENVELOPE_VERSION,
            run: run.clone(),
            seq,
            time: "2026-10-08T00:00:00.000Z".into(),
            event,
        }
    }

    fn whole_run(open: &Open) -> Vec<EventLine> {
        let run = RunId::derived(1, b"whole");
        vec![
            line(&run, 0, Event::Open(Box::new(open.clone()))),
            line(
                &run,
                1,
                Event::Result {
                    key: key("a"),
                    outcome: measured(),
                    location: None,
                },
            ),
            line(
                &run,
                2,
                Event::Result {
                    key: key("b"),
                    outcome: measured(),
                    location: None,
                },
            ),
            line(
                &run,
                3,
                Event::Close {
                    end: End::Complete,
                    conditions: None,
                },
            ),
        ]
    }

    #[test]
    fn a_whole_run_is_recorded_on_its_tree_and_a_second_delivery_changes_nothing() {
        let (_dir, store, kind, open, tree) = setup();
        let lines = whole_run(&open);
        let first = record(&store, &kind, &lines, &OpensOnly).expect("record");
        assert_eq!((first.written, first.already_present), (4, 0));
        let note = store.read_note(&kind, &tree).expect("read").expect("note");
        let again = record(&store, &kind, &lines, &OpensOnly).expect("again");
        assert_eq!((again.written, again.already_present), (0, 4), "idempotent");
        assert_eq!(
            store.read_note(&kind, &tree).expect("read").expect("note"),
            note
        );
        assert_eq!(fold(&note).runs.len(), 1);
    }

    #[test]
    fn a_batch_that_breaks_the_grammar_writes_nothing() {
        let (_dir, store, kind, open, tree) = setup();
        let mut lines = whole_run(&open);
        lines.remove(2); // b unanswered, yet closed complete
        lines[2].seq = 2;
        let error = record(&store, &kind, &lines, &OpensOnly).expect_err("refused");
        assert!(error.to_string().contains("unanswered"), "{error}");
        assert_eq!(store.read_note(&kind, &tree).expect("read"), None);
        let mut bad_time = whole_run(&open);
        bad_time[0].time = "2026-10-08T00:00:00Z".into();
        assert!(record(&store, &kind, &bad_time, &OpensOnly).is_err());
    }

    #[test]
    fn a_declare_after_a_result_is_refused_and_writes_nothing() {
        // The grammar is open · declare? · result* · close?: a declaration
        // after a result would change what the run's earlier lines mean.
        let (_dir, store, kind, open, tree) = setup();
        let lines = whole_run(&open);
        let run = lines[0].run.clone();
        record(&store, &kind, &lines[..2], &OpensOnly).expect("open and a result");
        let note = store.read_note(&kind, &tree).expect("read");
        let late = line(
            &run,
            2,
            Event::Declare {
                keys: None,
                metrics: BTreeMap::new(),
                build: None,
            },
        );
        let error =
            record(&store, &kind, &[lines[0].clone(), late], &OpensOnly).expect_err("refused");
        assert!(
            matches!(
                &error,
                RecordError::Refused(Refusal::Rule {
                    problem: Problem::DeclareAfterResult { seq: 2 },
                    ..
                })
            ),
            "{error}"
        );
        assert_eq!(store.read_note(&kind, &tree).expect("read"), note);
    }

    #[test]
    fn a_streamed_run_finds_its_tree_and_refuses_a_second_result() {
        let (_dir, store, kind, open, tree) = setup();
        let mut run = Run::open(&store, &kind, open).expect("open");
        run.result(&key("a"), measured(), None).expect("a");
        assert!(run.result(&key("a"), measured(), None).is_err());
        assert!(run.result(&key("zzz"), measured(), None).is_err());
        run.close(End::Refused(Reason::new("interrupted", None)), None)
            .expect("close");
        let folded = fold(&store.read_note(&kind, &tree).expect("read").expect("note"));
        assert!(folded.violations.is_empty(), "{:?}", folded.violations);
        assert_eq!(
            folded.runs[0].outcome(&key("b")),
            Some(Outcome::Refused(Reason::new("interrupted", None)))
        );
    }

    #[test]
    fn nothing_can_be_written_behind_a_close() {
        // A close that skips a seq would leave room to settle a refused key later.
        let (_dir, store, kind, open, tree) = setup();
        let lines = whole_run(&open);
        let gapped = [
            lines[0].clone(),
            lines[1].clone(),
            EventLine {
                seq: 5,
                ..lines[3].clone()
            },
        ];
        let error = record(&store, &kind, &gapped, &OpensOnly).expect_err("refused");
        assert!(error.to_string().contains("missing"), "{error}");
        assert_eq!(store.read_note(&kind, &tree).expect("read"), None);
        let ended = [
            lines[0].clone(),
            lines[1].clone(),
            line(
                &lines[0].run,
                2,
                Event::Close {
                    end: End::Refused(crate::format::event::Reason::new("noisy_host", None)),
                    conditions: None,
                },
            ),
        ];
        record(&store, &kind, &ended, &OpensOnly).expect("a refused end");
        let late = EventLine {
            seq: 3,
            ..lines[2].clone()
        };
        let error =
            record(&store, &kind, &[lines[0].clone(), late], &OpensOnly).expect_err("refused");
        assert!(error.to_string().contains("after close"), "{error}");
    }

    #[test]
    fn a_non_finite_number_is_refused_and_finite_ones_read_back_exactly() {
        use crate::format::event::{Try, Verdict, VerdictStatus};
        let (_dir, store, kind, open, tree) = setup();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut lines = whole_run(&open);
            lines[1].event = Event::Result {
                key: key("a"),
                outcome: Outcome::Verdict(Verdict {
                    status: VerdictStatus::Pass,
                    tries: vec![Try {
                        status: VerdictStatus::Pass,
                        duration_ms: Some(bad),
                        cpu_ms: None,
                    }],
                }),
                location: None,
            };
            let error = record(&store, &kind, &lines, &OpensOnly).expect_err("refused");
            assert!(error.to_string().contains("duration_ms"), "{error}");
            let Event::Result {
                outcome: Outcome::Measured(m),
                ..
            } = &mut lines[2].event
            else {
                panic!("measured")
            };
            m[0].value = 1.0;
            m[0].mad = Some(bad);
            lines[1] = whole_run(&open)[1].clone();
            let error = record(&store, &kind, &lines, &OpensOnly).expect_err("refused");
            assert!(error.to_string().contains("mad"), "{error}");
        }
        assert_eq!(
            store.read_note(&kind, &tree).expect("read"),
            None,
            "nothing written"
        );
        let mut lines = whole_run(&open);
        let exact = 0.1 + 0.2; // not representable as a short decimal
        let Event::Result {
            outcome: Outcome::Measured(m),
            ..
        } = &mut lines[1].event
        else {
            panic!("measured")
        };
        m[0].value = exact;
        record(&store, &kind, &lines, &OpensOnly).expect("record");
        let note = store.read_note(&kind, &tree).expect("read").expect("note");
        let back: EventLine = note
            .split(|b| *b == b'\n')
            .filter_map(|raw| serde_json::from_slice(raw).ok())
            .find(|l: &EventLine| l.seq == 1)
            .expect("seq 1");
        let Event::Result {
            outcome: Outcome::Measured(m),
            ..
        } = back.event
        else {
            panic!("measured")
        };
        assert_eq!(
            m[0].value.to_bits(),
            exact.to_bits(),
            "round-trips bit-exact"
        );
    }

    #[test]
    fn a_cpu_time_is_refused_unless_finite_and_non_negative_and_may_exceed_wall_time() {
        use crate::format::event::{Verdict, VerdictStatus};
        let (_dir, store, kind, open, tree) = setup();
        let with_cpu = |cpu: f64| {
            let mut lines = whole_run(&open);
            lines[1].event = Event::Result {
                key: key("a"),
                outcome: Outcome::Verdict(Verdict {
                    status: VerdictStatus::Pass,
                    tries: vec![Try {
                        status: VerdictStatus::Pass,
                        duration_ms: Some(10.0),
                        cpu_ms: Some(cpu),
                    }],
                }),
                location: None,
            };
            lines
        };
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = record(&store, &kind, &with_cpu(bad), &OpensOnly).expect_err("refused");
            assert!(
                matches!(&error, RecordError::Refused(Refusal::NonFinite { field, .. }) if field == "a try 0 cpu_ms"),
                "{error}"
            );
        }
        for bad in [-0.5, -1e9] {
            let error = record(&store, &kind, &with_cpu(bad), &OpensOnly).expect_err("refused");
            assert!(
                matches!(&error, RecordError::Refused(Refusal::Negative { field, .. }) if field == "a try 0 cpu_ms"),
                "{error}"
            );
        }
        assert_eq!(
            store.read_note(&kind, &tree).expect("read"),
            None,
            "nothing written"
        );
        // Parallel work: four cores busy for the try's 10 ms of wall time.
        record(&store, &kind, &with_cpu(40.0), &OpensOnly).expect("more CPU than wall time");
    }

    #[test]
    fn a_negative_wall_time_is_refused() {
        use crate::format::event::{Verdict, VerdictStatus};
        let (_dir, store, kind, open, tree) = setup();
        let mut lines = whole_run(&open);
        lines[1].event = Event::Result {
            key: key("a"),
            outcome: Outcome::Verdict(Verdict {
                status: VerdictStatus::Pass,
                tries: vec![Try {
                    status: VerdictStatus::Pass,
                    duration_ms: Some(-3.0),
                    cpu_ms: None,
                }],
            }),
            location: None,
        };
        let error = record(&store, &kind, &lines, &OpensOnly).expect_err("refused");
        assert!(
            matches!(&error, RecordError::Refused(Refusal::Negative { field, .. }) if field == "a try 0 duration_ms"),
            "{error}"
        );
        assert_eq!(
            store.read_note(&kind, &tree).expect("read"),
            None,
            "nothing written"
        );
    }

    #[test]
    fn later_events_of_an_unknown_run_are_refused() {
        let (_dir, store, kind, open, _) = setup();
        let lines = whole_run(&open);
        let error = record(&store, &kind, &lines[1..], &OpensOnly).expect_err("refused");
        assert!(error.to_string().contains("not recorded"), "{error}");
    }

    #[test]
    fn every_refusal_names_its_remedy() {
        // One example of every variant, and of every rule a run can break.
        let run = RunId::derived(1, b"r");
        let metric = MetricName::parse("m").expect("metric");
        let ns = Metric {
            unit: "ns".into(),
            better: None,
        };
        let problems = [
            Problem::Unreadable("x".into()),
            Problem::ConflictingSeq { seq: 1 },
            Problem::Empty,
            Problem::Envelope { seq: 0, found: 1 },
            Problem::FirstMissing,
            Problem::FirstNotOpen,
            Problem::Time { seq: 1 },
            Problem::Gap {
                seq: 3,
                from: 2,
                to: 2,
                read_to: 1,
            },
            Problem::AfterClose { seq: 1 },
            Problem::SecondOpen { seq: 1 },
            Problem::SecondDeclare { seq: 1 },
            Problem::DeclareAfterResult { seq: 2 },
            Problem::KeysTwice { seq: 1 },
            Problem::UndeclaredMetric {
                seq: 1,
                key: key("a"),
                metric: metric.clone(),
            },
            Problem::SecondResult { seq: 1 },
            Problem::UndeclaredKey { seq: 1 },
            Problem::IncompleteClose {
                seq: 1,
                unanswered: 1,
            },
            Problem::ReservedMetric {
                seq: 1,
                metric: metric.clone(),
            },
            Problem::MetricRedefined {
                seq: 1,
                metric: metric.clone(),
                was: ns.clone(),
                now: ns,
            },
            Problem::NoUnit { seq: 1, metric },
        ];
        let tree = TreeId::parse(&"a".repeat(40)).expect("tree");
        let commit = CommitId::parse(&"b".repeat(40)).expect("commit");
        let refusals = [
            Refusal::Envelope {
                run: run.clone(),
                found: 1,
            },
            Refusal::Time {
                run: run.clone(),
                seq: 0,
                time: "x".into(),
            },
            Refusal::NonFinite {
                run: run.clone(),
                seq: 0,
                field: "v".into(),
            },
            Refusal::Negative {
                run: run.clone(),
                seq: 0,
                field: "v".into(),
            },
            Refusal::UnknownCommit {
                run: run.clone(),
                commit,
            },
            Refusal::OtherTree {
                run: run.clone(),
                recorded: tree,
            },
            Refusal::NoOpen { run: run.clone() },
            Refusal::NotRecorded { run: run.clone() },
            Refusal::Ended { run: run.clone() },
            Refusal::OverBudget {
                run: run.clone(),
                kind: Kind::parse("perf").expect("kind"),
                budget: 1,
                bytes: 2,
            },
        ]
        .into_iter()
        .chain(problems.into_iter().map(|problem| Refusal::Rule {
            run: Some(run.clone()),
            problem,
        }));
        for refusal in refusals {
            let remedy = refusal.remedy();
            assert!(!remedy.is_empty(), "{refusal:?}");
            assert!(refusal.to_string().ends_with(remedy), "{refusal}");
        }
    }
}
