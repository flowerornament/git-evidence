//! The Rust API, `Evidence`: write a run, finish it in one commit, resume
//! one after a crash, and ask at one pin.

mod common;

use common::*;
use git_evidence::{
    Direction, End, Evidence, Metric, MetricName, Outcome, Reason, Selector, TrendQuery, Try,
    Verdict, VerdictStatus,
};

fn evidence(f: &Fixture) -> Evidence {
    Evidence::open(f.git.git_dir()).trunk("refs/heads/master")
}

fn declaring_ns(mut open: git_evidence::Open) -> git_evidence::Open {
    open.suite.metrics.insert(
        MetricName::parse("block.p99_ns").expect("metric"),
        Metric {
            unit: "ns".into(),
            better: Some(Direction::Lower),
        },
    );
    open
}

#[test]
fn a_run_written_through_evidence_is_asked_back_with_its_meaning() {
    let f = fixture(2);
    let evidence = evidence(&f);
    for (landing, value) in [(0, 80.0), (1, 82.0)] {
        let mut run = evidence
            .run(
                &kind("perf"),
                declaring_ns(open(
                    &f.landings[landing],
                    &Shape::default(),
                    &["parse_large"],
                )),
            )
            .expect("open");
        run.result(&key("parse_large"), p99(value), None)
            .expect("result");
        run.close(End::Complete, None).expect("close");
    }
    let points = evidence
        .at(|at| {
            at.trend(&TrendQuery {
                landings: Selector::newest(10.try_into().expect("not zero")),
                ..TrendQuery::new(
                    "block.p99_ns".parse().expect("metric"),
                    "parse_large".parse().expect("key"),
                )
            })
        })
        .expect("ask");
    assert_eq!(points[0].unit, "ns");
    assert_eq!(points.len(), 2);
    // Two questions inside one `at` read one snapshot.
    let (first, trend, second) = evidence
        .at(|at| {
            let first = at.pin().clone();
            let trend = at.trend(&git_evidence::TrendQuery {
                metric: "block.p99_ns".parse().expect("name"),
                key: "parse_large".parse().expect("name"),
                machine: None,
                landings: newest(10),
            })?;
            at.keys(&git_evidence::KeysQuery::default())?;
            Ok((first, trend, at.pin().clone()))
        })
        .expect("at");
    assert_eq!(first, second);
    assert_eq!(trend, points);
}

#[test]
fn finish_writes_the_rest_of_a_run_in_one_commit_or_nothing() {
    let f = fixture(1);
    let evidence = evidence(&f);
    let mut opened = open(&f.landings[0], &Shape::default(), &[]);
    opened.keys = None;
    let notes = || {
        f.git
            .output(&["rev-list", "--count", "refs/notes/evidence/gates"])
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .expect("count")
    };
    let pass = |ms: f64| {
        Outcome::Verdict(Verdict {
            status: VerdictStatus::Pass,
            tries: vec![Try {
                status: VerdictStatus::Pass,
                duration_ms: Some(ms),
                cpu_ms: None,
            }],
        })
    };
    // A batch naming an undeclared key is refused whole, and the run continues.
    let run = evidence.run(&kind("gates"), opened.clone()).expect("open");
    let id = run.id().clone();
    let before = notes();
    let refused = run
        .finish()
        .declare(
            Some(vec![key("fmt")]),
            std::collections::BTreeMap::default(),
            None,
        )
        .results([(key("fmt"), pass(1.0)), (key("clippy"), pass(2.0))])
        .end(End::Complete);
    assert!(refused.is_err(), "clippy was never declared");
    assert_eq!(notes(), before, "nothing was written");
    // Resumed, the run declares, answers and ends in one commit.
    let resumed = evidence.resume(&kind("gates"), &id).expect("resume");
    assert_eq!(
        (resumed.declared.clone(), resumed.answered.len()),
        (None, 0)
    );
    resumed
        .run
        .finish()
        .declare(
            Some(vec![key("fmt"), key("clippy")]),
            std::collections::BTreeMap::default(),
            None,
        )
        .results([(key("fmt"), pass(1.0)), (key("clippy"), pass(2.0))])
        .conditions(serde_json::json!({"tests_observed": 2}))
        .end(End::Complete)
        .expect("finish");
    assert_eq!(
        notes().parse::<u32>().expect("n"),
        before.parse::<u32>().expect("n") + 1,
        "one commit"
    );
    // The derived durations read back, marked derived.
    let detail = evidence.at(|at| at.run(&id)).expect("show");
    let fmt = detail
        .outcomes
        .iter()
        .find(|o| o.key.as_str() == "fmt")
        .expect("fmt");
    let derived = &fmt.outcome.measures()[0];
    assert_eq!(derived.origin, git_evidence::MeasureOrigin::Derived);
    assert_eq!(derived.metric.as_str(), "verdict.pass.duration_ms");
}

#[test]
fn a_closed_run_cannot_be_resumed_and_an_unknown_one_is_named() {
    let f = fixture(1);
    let evidence = evidence(&f);
    let mut run = evidence
        .run(
            &kind("perf"),
            declaring_ns(open(&f.landings[0], &Shape::default(), &["parse_large"])),
        )
        .expect("open");
    let id = run.id().clone();
    run.result(&key("parse_large"), p99(1.0), None)
        .expect("result");
    let end = End::Refused(Reason::new("interrupted", Some("SIGTERM after 30 s")));
    run.close(end.clone(), None).expect("close");
    // The end reads back whole, its detail with it.
    let shown = evidence.at(|at| at.run(&id)).expect("show");
    assert_eq!(shown.end, Some(end));
    assert!(evidence.resume(&kind("perf"), &id).is_err(), "it ended");
    assert!(
        evidence
            .resume(&kind("perf"), &git_evidence::RunId::new())
            .is_err(),
        "never recorded"
    );
}

#[test]
fn questions_inside_at_never_wait_on_a_lock() {
    // `at` answers in a read snapshot: asking Evidence again inside it (or
    // another process catching up meanwhile) is not blocked by it. The outer
    // snapshot stays open until the inner question returns, so an inner
    // question waiting on it could only fail or never return: its answer is
    // the witness, and the hang guard turns never into a failure.
    let f = fixture(1);
    let evidence = evidence(&f);
    let (done, answered) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let pins = evidence.at(|at| {
            let inner = evidence.at(|at| Ok(at.pin().clone()))?;
            Ok((at.pin().clone(), inner))
        });
        done.send(pins).ok();
    });
    let (outer, inner) = answered
        .recv_timeout(git_evidence::testing::HANG_GUARD)
        .expect("the nested question returns")
        .expect("nested");
    assert_eq!(outer, inner);
}

#[test]
fn a_run_over_its_kinds_budget_is_refused_and_check_says_so_first() {
    // Evidence is permanent, so a run's size is bounded per kind, and a
    // producer can see its batch's size before sending it.
    let f = fixture(1);
    let evidence = evidence(&f);
    let lines = run_lines(
        declaring_ns(open(&f.landings[0], &Shape::default(), &["parse_large"])),
        &[("parse_large", p99(1.0))],
        Some(End::Complete),
    );
    let planned = evidence
        .record_dry_run(&kind("perf"), &lines)
        .expect("check");
    let size = planned.runs[&lines[0].run].run_bytes;
    assert_eq!(planned.bytes_added, size, "a fresh run is all new bytes");
    let set = |value: String| {
        assert!(
            f.git
                .output(&["config", "evidence.perf.budget", &value])
                .expect("git")
                .status
                .success()
        );
    };
    set((size - 1).to_string());
    let refused = evidence
        .record(&kind("perf"), &lines)
        .expect_err("over budget");
    assert!(
        matches!(
            refused,
            git_evidence::Error::Record(git_evidence::RecordError::Refused(
                git_evidence::Refusal::OverBudget { .. }
            ))
        ),
        "{refused}"
    );
    assert_eq!(
        f.git
            .output(&["notes", "--ref", "refs/notes/evidence/perf", "list"])
            .expect("git")
            .stdout,
        b""
    );
    set(size.to_string());
    let recorded = evidence
        .record(&kind("perf"), &lines)
        .expect("within budget");
    assert_eq!(recorded.bytes_added, size);
    let note = note(
        &f,
        &git_evidence::Kind::parse("perf").expect("kind"),
        &tree(&f, &f.landings[0]),
    )
    .expect("note");
    assert_eq!(
        note.len() as u64,
        size,
        "the bytes reported are the bytes written"
    );
}

#[test]
fn a_returned_run_is_a_whole_recordable_run_after_every_write_and_never_in_this_store() {
    // A measuring host holds no evidence: its run goes to a file the caller's
    // store records. From the open on, the file is a run that store accepts;
    // a refused write leaves the file as it was; this store gains nothing.
    let f = fixture(1);
    let host = evidence(&f);
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("perf").join("run.ndjson");
    let read = || -> Vec<git_evidence::EventLine> {
        std::fs::read_to_string(&path)
            .expect("file")
            .lines()
            .map(|line| serde_json::from_str(line).expect("line"))
            .collect()
    };
    let mut opened = declaring_ns(open(&f.landings[0], &Shape::default(), &[]));
    opened.keys = None;
    let mut run = host
        .run_to_file(&kind("perf"), opened, &path)
        .expect("open");
    let id = run.id().clone();
    let begun = read();
    assert_eq!(begun.len(), 1, "the open is returned before any work");
    run.declare(
        Some(vec![key("parse_large")]),
        std::collections::BTreeMap::default(),
        None,
    )
    .expect("declare");
    let before = read();
    let refused = run.result(&key("additive"), p99(1.0), None);
    assert!(refused.is_err(), "additive was never declared");
    assert_eq!(read(), before, "a refused write leaves the file whole");
    run.finish()
        .results([(key("parse_large"), p99(80.0))])
        .end(End::Complete)
        .expect("finish");
    let lines = read();
    assert_eq!(
        lines.iter().map(|l| l.seq).collect::<Vec<_>>(),
        [0, 1, 2, 3],
        "numbering continues past the refusal"
    );
    assert_eq!(
        f.git
            .output(&["notes", "--ref", "refs/notes/evidence/perf", "list"])
            .expect("git")
            .stdout,
        b"",
        "the host's store holds none of it"
    );
    // The caller records the file as it is, and asks it back.
    host.record(&kind("perf"), &lines)
        .expect("the caller records it");
    let detail = host.at(|at| at.run(&id)).expect("show");
    assert_eq!(detail.run, id);
}
