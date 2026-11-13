//! The bench's store: a fresh repository of a declared size (`StoreSize`),
//! landings on `master` and runs recorded on them, built the same way on every
//! machine so two benches of one size measure one store.
//!
//! ```text
//! landing n: tree { landing }
//! runs:      run i on landing ⌊i·L/R⌋; even i perf (16 measured keys), odd i checks (8 verdicts)
//! ```
//!
//! Git's `fast-import` writes it in two passes (the landings, then one notes
//! commit per kind), seconds where `record`'s one `git notes` per tree would
//! take minutes. The notes hold exactly the lines `record` writes for the same
//! runs, byte for byte: the pair is compared by
//! `bulk_notes_are_the_notes_record_writes` below. The store is the bench's
//! own temporary directory, never a store anyone reads.

use super::store::{COMMITTER_EMAIL, COMMITTER_NAME};
use super::{BenchError, StoreSize};
use crate::format::event::{
    Direction, ENVELOPE_VERSION, End, Event, EventLine, Measure, Metric, Open, Outcome, Try,
    Verdict, VerdictStatus,
};
use crate::format::id::{CommitId, Key, Kind, Machine, MetricName, ProducerName, RunId, TreeId};
use git_process::Git;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The trunk of a generated store.
pub(super) const TRUNK: &str = "refs/heads/master";
/// The machine every generated run names.
const MACHINE: &str = "generated";
/// Keys of one perf run, and of one checks run.
const PERF_KEYS: u32 = 16;
const CHECK_KEYS: u32 = 8;
/// The metric each perf key measures.
const METRIC: &str = "latency_ns";
/// Landing 0's time, in Unix milliseconds; each later landing is ten minutes on.
const EPOCH_MS: u64 = 1_700_000_000_000;
const LANDING_MS: u64 = 600_000;

fn perf_key(k: u32) -> String {
    format!("k{k:02}")
}

/// Writes a store of `size` into the empty bare repository `git`: its
/// landings on [`TRUNK`], then its runs.
pub(super) fn generate(git: &Git, size: StoreSize) -> Result<(), BenchError> {
    git.run(&["fast-import", "--quiet", "--done"], Some(&landings(size)))?;
    let listing = git.run(
        &[
            "log",
            "--reverse",
            "--first-parent",
            "--format=%H %T",
            TRUNK,
        ],
        None,
    )?;
    let landed = String::from_utf8_lossy(&listing)
        .lines()
        .map(|line| {
            let (commit, tree) = line.split_once(' ').unwrap_or((line, ""));
            Ok((CommitId::parse(commit)?, TreeId::parse(tree)?))
        })
        .collect::<Result<Vec<_>, crate::format::id::IdError>>()?;
    let mut stream = Vec::new();
    for (kind, notes) in notes(&landed, size)? {
        write!(
            text(&mut stream),
            "commit refs/notes/evidence/{kind}\ncommitter {COMMITTER_NAME} <{COMMITTER_EMAIL}> {} +0000\ndata 12\nbench notes\n",
            EPOCH_MS / 1000
        )
        .ok();
        for (tree, lines) in notes {
            let body = note(&lines)?;
            let tree = tree.as_str();
            write!(
                text(&mut stream),
                "M 100644 inline {}/{}\ndata {}\n",
                &tree[..2],
                &tree[2..],
                body.len()
            )
            .ok();
            stream.extend(body);
            stream.push(b'\n');
        }
    }
    stream.extend_from_slice(b"done\n");
    git.run(&["fast-import", "--quiet", "--done"], Some(&stream))?;
    Ok(())
}

/// A `fast-import` stream of `size.landings` commits on [`TRUNK`], each with
/// its own tree.
fn landings(size: StoreSize) -> Vec<u8> {
    let mut stream = String::new();
    for n in 0..u64::from(size.landings.get()) {
        let title = format!("landing {n}\n");
        write!(
            stream,
            "commit {TRUNK}\ncommitter {COMMITTER_NAME} <{COMMITTER_EMAIL}> {} +0000\ndata {len}\n{title}\
             M 100644 inline landing\ndata {len}\n{title}\n",
            (EPOCH_MS + n * LANDING_MS) / 1000,
            len = title.len(),
        )
        .ok();
    }
    stream.push_str("done\n");
    stream.into_bytes()
}

/// Each kind's notes: the lines of every run on each landing's tree.
type Notes = BTreeMap<Kind, BTreeMap<TreeId, Vec<EventLine>>>;

fn notes(landed: &[(CommitId, TreeId)], size: StoreSize) -> Result<Notes, BenchError> {
    let mut notes = Notes::new();
    for i in 0..size.runs {
        // Spread evenly: run i on landing ⌊i·L/R⌋, always below L.
        let n = u64::from(i) * u64::from(size.landings.get()) / u64::from(size.runs);
        let n = u32::try_from(n).expect("below the landings, a u32");
        let (commit, tree) = &landed[n as usize];
        let (kind, lines) = run(i, n, commit)?;
        notes
            .entry(kind)
            .or_default()
            .entry(tree.clone())
            .or_default()
            .extend(lines);
    }
    // `record` writes a batch's runs in run id order; the fold reads any order.
    for trees in notes.values_mut() {
        for lines in trees.values_mut() {
            lines.sort_by(|a, b| a.run.cmp(&b.run));
        }
    }
    Ok(notes)
}

/// Run `i`, on landing `n` at `commit`: its kind and its lines.
fn run(i: u32, n: u32, commit: &CommitId) -> Result<(Kind, Vec<EventLine>), BenchError> {
    let perf = i.is_multiple_of(2);
    let kind = Kind::parse(if perf { "perf" } else { "checks" })?;
    let opened = EPOCH_MS + u64::from(n) * LANDING_MS + u64::from(i % 2) * 1000;
    let id = RunId::derived(u128::from(opened), format!("bench run {i}").as_bytes());
    Ok((kind, lines(&id, opened, perf, n, commit)?))
}

/// A perf run of the generated shape on `commit`, under a new run id and the
/// time now: what the bench's `record` figure writes.
pub(super) fn fresh_run(commit: &CommitId) -> Result<Vec<EventLine>, BenchError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let opened = u64::try_from(now.as_millis()).unwrap_or(EPOCH_MS);
    lines(&crate::format::id::RunId::new(), opened, true, 0, commit)
}

/// The lines of one run opened at `opened` (Unix milliseconds): perf, with
/// 16 measured keys, or checks, with 8 passing verdicts; `n` varies the values.
fn lines(
    id: &RunId,
    opened: u64,
    perf: bool,
    n: u32,
    commit: &CommitId,
) -> Result<Vec<EventLine>, BenchError> {
    let mut open = Open::new(
        commit.clone(),
        ProducerName::parse(if perf { "bench-suite" } else { "bench-checks" })?,
        if perf {
            "bench-suite/1"
        } else {
            "bench-checks/1"
        },
        &Machine::parse(MACHINE)?,
    );
    let mut results = Vec::new();
    if perf {
        open.suite.metrics.insert(
            MetricName::parse(METRIC)?,
            Metric {
                unit: "ns".into(),
                better: Some(Direction::Lower),
            },
        );
        for k in 0..PERF_KEYS {
            // A quiet series: within 4% of 1000.
            let value = 1000.0 + f64::from((n % 1000 * 7 + k * 13) % 40);
            let mut measure = Measure::new(MetricName::parse(METRIC)?, value)?;
            measure.n = Some(5);
            measure.mad = Some(1.0);
            results.push((Key::parse(&perf_key(k))?, Outcome::Measured(vec![measure])));
        }
    } else {
        for k in 0..CHECK_KEYS {
            let duration = 100.0 + f64::from((n % 1000 + k) % 20);
            let tries = vec![Try {
                status: VerdictStatus::Pass,
                duration_ms: Some(duration),
                cpu_ms: Some(duration * 2.0),
            }];
            results.push((
                Key::parse(&format!("check{k}"))?,
                Outcome::Verdict(Verdict {
                    status: VerdictStatus::Pass,
                    tries,
                }),
            ));
        }
    }
    open.keys = Some(results.iter().map(|(key, _)| key.clone()).collect());
    let mut events = vec![Event::Open(Box::new(open))];
    events.extend(results.into_iter().map(|(key, outcome)| Event::Result {
        key,
        outcome,
        location: None,
    }));
    events.push(Event::Close {
        end: End::Complete,
        conditions: None,
    });
    let lines = events
        .into_iter()
        .zip(0u32..)
        .map(|(event, seq)| EventLine {
            v: ENVELOPE_VERSION,
            run: id.clone(),
            seq,
            time: git_process::Timestamp::from_unix_millis(u128::from(opened + u64::from(seq)))
                .into(),
            event,
        })
        .collect();
    Ok(lines)
}

/// A note's bytes as `record` writes them: each line's JSON and a newline.
fn note(lines: &[EventLine]) -> Result<Vec<u8>, BenchError> {
    let mut body = Vec::new();
    for line in lines {
        body.extend(
            serde_json::to_vec(line).map_err(|e| BenchError::Unreadable("run", e.to_string()))?,
        );
        body.push(b'\n');
    }
    Ok(body)
}

/// A byte buffer written as text.
fn text(bytes: &mut Vec<u8>) -> impl std::fmt::Write + '_ {
    struct Text<'a>(&'a mut Vec<u8>);
    impl std::fmt::Write for Text<'_> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
    Text(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::store::init_bare;
    use crate::evidence::Evidence;
    use crate::store::Store;
    use std::num::NonZeroU32;

    fn size(landings: u32, runs: u32) -> StoreSize {
        StoreSize {
            landings: NonZeroU32::new(landings).expect("landings"),
            runs,
        }
    }

    #[test]
    fn bulk_notes_are_the_notes_record_writes() {
        // The pair: the bulk writer against `record`, the one write path, on
        // the same landings (generation is deterministic, so the commits match).
        let dir = tempfile::tempdir().expect("tempdir");
        let bulk = init_bare(&dir.path().join("bulk.git")).expect("bulk");
        generate(&bulk, size(5, 13)).expect("generate");
        let recorded = init_bare(&dir.path().join("recorded.git")).expect("recorded");
        generate(&recorded, size(5, 0)).expect("landings only");
        let listing = recorded
            .run(&["log", "--reverse", "--format=%H %T", TRUNK], None)
            .expect("log");
        let landed: Vec<(CommitId, TreeId)> = String::from_utf8_lossy(&listing)
            .lines()
            .map(|line| {
                let (c, t) = line.split_once(' ').expect("pair");
                (CommitId::parse(c).expect("c"), TreeId::parse(t).expect("t"))
            })
            .collect();
        assert_eq!(landed.len(), 5);
        let evidence = Evidence::open(recorded.git_dir());
        let mut written = 0;
        for (kind, trees) in notes(&landed, size(5, 13)).expect("notes") {
            // These independent runs compare final note bytes, not arrival
            // order. Record one batch per kind, keeping every tree below.
            let lines: Vec<_> = trees.into_values().flatten().collect();
            evidence.record(&kind, &lines).expect("record");
            written += lines.len();
        }
        assert!(written > 13 * 3, "every run was recorded");
        let (bulk_store, recorded_store) =
            (Store::open(bulk.git_dir()), Store::open(recorded.git_dir()));
        for kind in ["perf", "checks"] {
            let kind = Kind::parse(kind).expect("kind");
            for (_, tree) in &landed {
                assert_eq!(
                    bulk_store.read_note(&kind, tree).expect("bulk"),
                    recorded_store.read_note(&kind, tree).expect("recorded"),
                    "{kind} on {tree}"
                );
            }
        }
    }

    #[test]
    fn a_generated_store_has_its_size_and_reads_without_violations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = init_bare(&dir.path().join("store.git")).expect("store");
        generate(&git, size(7, 14)).expect("generate");
        let evidence = Evidence::open(git.git_dir()).trunk(TRUNK);
        let rows = evidence
            .sql("SELECT (SELECT count(*) FROM landings), (SELECT count(*) FROM runs WHERE ended = 'complete'), (SELECT count(*) FROM all_violations)")
            .expect("sql");
        assert_eq!(
            rows.rows,
            vec![vec![
                crate::ask::sql::SqlValue::Integer(7),
                crate::ask::sql::SqlValue::Integer(14),
                crate::ask::sql::SqlValue::Integer(0)
            ]]
        );
    }
}
