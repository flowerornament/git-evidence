//! Durability by real process death (README "Guarantees"): a child process records
//! and aborts at a named point (the `fault-injection` feature) or between calls;
//! the parent then reads what survived. A record is all there or not there; a
//! streamed run's acknowledged events stand; the producer ends what it left open.
//! An index rebuild by another build is all there or not there.

mod common;

use common::*;
use git_evidence::Evidence;
use git_evidence::RunId;
use git_evidence::{End, EventLine, Reason};
use std::path::Path;
use std::process::Command;

/// The batch every scenario records: one whole run on the newest landing.
fn batch(f: &Fixture) -> Vec<EventLine> {
    let lines = run_lines(
        open(
            f.landings.last().expect("landing"),
            &Shape::default(),
            &["a", "b"],
        ),
        &[("a", p99(1.0)), ("b", p99(2.0))],
        Some(End::Complete),
    );
    // A fixed id and fixed times, so parent and child build the same lines.
    let run = RunId::derived(1, b"crash");
    lines
        .into_iter()
        .map(|mut line| {
            line.run = run.clone();
            line.time = format!("2026-10-08T00:00:0{}.000Z", line.seq);
            line
        })
        .collect()
}

/// The child: runs one scenario against `CRASH_REPO` and dies where it says.
#[test]
fn child() {
    let (Ok(scenario), Ok(dir)) = (std::env::var("CRASH_SCENARIO"), std::env::var("CRASH_DIR"))
    else {
        return; // an ordinary test run: nothing to do
    };
    let evidence = Evidence::open(Path::new(&dir).join(".git"));
    let landing = git_process::Git::at(evidence.git_dir())
        .resolve("refs/heads/master")
        .expect("resolve")
        .expect("master");
    let commit = git_evidence::CommitId::parse(landing.as_str()).expect("commit");
    match scenario.as_str() {
        "record" => {
            let lines: Vec<EventLine> =
                serde_json::from_str(&std::env::var("CRASH_LINES").expect("lines")).expect("json");
            evidence.record(&kind("perf"), &lines).expect("record");
        }
        "stream" => {
            let mut run = evidence
                .run(
                    &kind("perf"),
                    declaring(
                        open(&commit, &Shape::default(), &["a", "b"]),
                        &Shape::default(),
                        &[("a", p99(1.0))],
                    ),
                )
                .expect("open");
            run.result(&key("a"), p99(1.0)).expect("a");
            std::process::abort(); // measuring b when the machine died
        }
        "refresh" => {
            evidence
                .trunk("refs/heads/master")
                .sql_path()
                .expect("refresh");
        }
        other => panic!("unknown scenario {other}"),
    }
}

/// Runs the child scenario on the batch every scenario records.
fn crash(f: &Fixture, scenario: &str, fault: Option<&str>) {
    crash_with(f, scenario, fault, &batch(f));
}

/// Runs the child scenario, which must die by SIGABRT, recording `lines`.
fn crash_with(f: &Fixture, scenario: &str, fault: Option<&str>, lines: &[EventLine]) {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", "child", "--nocapture", "--test-threads", "1"])
        .env("CRASH_SCENARIO", scenario)
        .env("CRASH_DIR", f.dir.path())
        .env("CRASH_LINES", serde_json::to_string(lines).expect("json"));
    match fault {
        Some(fault) => command.env("GIT_EVIDENCE_FAULT", fault),
        None => command.env_remove("GIT_EVIDENCE_FAULT"),
    };
    let status = command.output().expect("child").status;
    // An abort, not a panic: a panic before the fault point would write nothing
    // and pass "nothing written" for the wrong reason.
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(6),
        "the child must abort (SIGABRT) in scenario {scenario}: {status:?}"
    );
}

fn note_on_last(f: &Fixture) -> Option<Vec<u8>> {
    note(f, &perf(), &tree(f, f.landings.last().expect("landing")))
}

#[test]
fn a_record_killed_after_its_checks_writes_nothing() {
    let f = fixture(1);
    crash(&f, "record", Some("record:checked"));
    assert_eq!(note_on_last(&f), None);
}

#[test]
fn a_batch_over_two_trees_killed_midway_writes_neither() {
    let f = fixture(2);
    let mut lines = batch(&f);
    let mut other = run_lines(
        open(&f.landings[0], &Shape::default(), &["a"]),
        &[("a", p99(3.0))],
        Some(End::Complete),
    );
    for line in &mut other {
        line.run = RunId::derived(2, b"crash");
        line.time = format!("2026-10-08T00:01:0{}.000Z", line.seq);
    }
    lines.extend(other);
    // Dies after the first tree's note is staged, before the ref moves.
    crash_with(&f, "record", Some("append:staged@1"), &lines);
    for landing in &f.landings {
        assert_eq!(
            note(&f, &perf(), &tree(&f, landing)),
            None,
            "all or nothing"
        );
    }
    let again = f.evidence.record(&perf(), &lines).expect("redeliver");
    assert_eq!(
        again.written,
        lines.len(),
        "the leftover staging ref is harmless"
    );
}

#[test]
fn a_record_killed_after_its_ref_moved_is_durable_and_redelivery_changes_nothing() {
    let f = fixture(1);
    crash(&f, "record", Some("append:ref-moved"));
    let kept = note_on_last(&f).expect("the lines survived the abort");
    let again = f.evidence.record(&perf(), &batch(&f)).expect("redeliver");
    assert_eq!((again.written, again.already_present), (0, 4), "idempotent");
    assert_eq!(note_on_last(&f), Some(kept));
    let runs = ask(&f, every_run);
    let ended: Vec<_> = runs.iter().map(|r| r.end.clone()).collect();
    assert_eq!(ended, vec![Some(End::Complete)], "one run, whole");
}

#[test]
fn a_streamed_run_killed_midway_keeps_its_acknowledged_results_and_is_ended_by_its_producer() {
    let f = fixture(1);
    crash(&f, "stream", None);
    let runs = ask(&f, every_run);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0].end, None, "the run is still open");
    // The producer, restarting, finds its unfinished run and ends it.
    // It knows its run by its own id; here, the one run the note holds.
    let text = String::from_utf8(note_on_last(&f).expect("note")).expect("utf8");
    let run = text
        .lines()
        .map(|line| serde_json::from_str::<EventLine>(line).expect("event"))
        .next()
        .expect("a line")
        .run;
    let resumed = f.evidence.resume(&kind("perf"), &run).expect("resume");
    resumed
        .run
        .close(End::Refused(Reason::new("interrupted")))
        .expect("close");
    let runs = ask(&f, every_run);
    assert_eq!(runs[0].end, Some(End::Refused(Reason::new("interrupted"))));
    assert_eq!(
        runs[0].outcomes.get("measured"),
        Some(&1),
        "a stands; b was never measured"
    );
}

/// A file another build folded is rebuilt in one write transaction: killed
/// before it commits, the file still holds that build's complete fold, never
/// empty tables an external reader (`sql --path`) would take for a store
/// without landings.
#[test]
fn a_rebuild_for_another_builds_file_killed_before_commit_leaves_the_previous_fold() {
    let f = fixture(1);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse", p99(100.0))],
        Some(End::Complete),
    );
    let counts = "SELECT (SELECT count(*) FROM trunk), (SELECT count(*) FROM all_measures),
                         (SELECT value FROM meta WHERE key = 'source')";
    let committed = || {
        let db = rusqlite::Connection::open_with_flags(
            cache(&f),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("index");
        db.query_row(counts, [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("committed rows")
    };
    f.evidence.sql_path().expect("refresh");
    rusqlite::Connection::open(cache(&f))
        .expect("index")
        .execute(
            "UPDATE meta SET value = 'sha256:another-build' WHERE key = 'source'",
            [],
        )
        .expect("another build's file");
    crash(&f, "refresh", Some("index:folded"));
    assert_eq!(committed(), (1, 1, "sha256:another-build".to_owned()));
    f.evidence.sql_path().expect("refresh");
    let (landings, measures, source) = committed();
    assert_eq!((landings, measures), (1, 1));
    assert_ne!(source, "sha256:another-build", "rebuilt by this build");
}
