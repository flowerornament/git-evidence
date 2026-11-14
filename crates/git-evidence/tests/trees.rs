//! Evidence is keyed by tree: a candidate that lands unchanged carries its
//! evidence, a run filed on another tree is in no answer, and a run on a
//! commit the store lacks is refused.

mod common;

use common::*;
use git_evidence::End;
use git_evidence::LogQuery;
use git_evidence::NotRecorded;

/// A run of `source` on landing 0, as an importer would project it.
fn projection(f: &Fixture, source: &str, outcome: git_evidence::Outcome) {
    let opened = open(&f.landings[0], &Shape::default(), &["parse_large"]).source(source);
    let lines = run_lines(opened, &[("parse_large", outcome)], Some(End::Complete));
    f.evidence.record(&perf(), &lines).expect("record");
}

#[test]
fn a_candidate_that_lands_unchanged_carries_its_evidence() {
    // Evidence is keyed by tree: a run on a candidate commit with the landing's
    // tree is the landing's evidence, with no matching rule.
    let f = fixture(2);
    let landing_tree = f.git.tree_of(f.landings[1].object()).expect("tree");
    let candidate = f
        .git
        .write_commit(&landing_tree, &[f.landings[0].object()], "the candidate")
        .expect("candidate");
    let lines = run_lines(
        open(&candidate, &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(77.0))],
        Some(End::Complete),
    );
    f.evidence.record(&perf(), &lines).expect("record");
    let points = ask(&f, |at| at.trend(&parse_large_trend(1)));
    assert_eq!(points[0].landing.get(), 1);
    let entries = ask(&f, |at| at.log(&LogQuery::tree(&tree(&f, &f.landings[1]))));
    assert_eq!(entries.len(), 1);
}

#[test]
fn a_misfiled_run_is_in_no_answer() {
    // A run opened for landing 0's commit but filed on landing 1's tree: its
    // value is in no trend and no show.
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse_large", p99(80.0))],
        Some(End::Complete),
    );
    let lines = run_lines(
        open(&f.landings[0], &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(990.0))],
        Some(End::Complete),
    );
    let text: Vec<String> = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect();
    let status = git_process::testing::hermetic(&mut std::process::Command::new("git"))
        .arg("--git-dir")
        .arg(f.git.git_dir())
        .args([
            "notes",
            "--ref",
            "refs/notes/evidence/perf",
            "append",
            "--no-separator",
            "-m",
        ])
        .arg(text.join("\n"))
        .arg(tree(&f, &f.landings[1]).as_str())
        .status()
        .expect("git");
    assert!(status.success());
    let points = ask(&f, |at| at.trend(&parse_large_trend(2)));
    let values: Vec<f64> = points.iter().map(|p| p.value).collect();
    assert_eq!(values, vec![80.0], "{points:?}");
    let shown = not_recorded(&f, |at| at.run(&lines[0].run));
    assert!(matches!(shown, NotRecorded::Unreadable { .. }), "{shown:?}");
}

#[test]
fn a_run_on_a_commit_not_in_the_store_is_refused() {
    let f = fixture(1);
    let missing = git_evidence::CommitId::parse(&"ab".repeat(20)).expect("commit");
    let lines = run_lines(
        open(&missing, &Shape::default(), &["parse_large"]),
        &[],
        None,
    );
    let error = f.evidence.record(&perf(), &lines).expect_err("refused");
    assert!(
        matches!(
            error,
            git_evidence::Error::Record(git_evidence::RecordError::Refused(
                git_evidence::Refusal::UnknownCommit { .. }
            ))
        ),
        "{error}"
    );
}

#[test]
fn a_source_recorded_twice_is_two_runs_each_a_row() {
    let f = fixture(1);
    projection(&f, "ledger:row#7", p99(80.0));
    projection(&f, "ledger:row#7", p99(80.0));
    let points = ask(&f, |at| at.trend(&parse_large_trend(1)));
    let values: Vec<f64> = points.iter().map(|p| p.value).collect();
    assert_eq!(values, vec![80.0, 80.0]);
}
