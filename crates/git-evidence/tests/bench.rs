//! `git evidence bench`: it times every verb it
//! declares, on a generated store or a copy of this one, and the run it
//! returns records as kind `tool`, its numbers readable with `sql`.
mod common;

use common::*;
use std::process::Command;

/// The command line, never reading the repository the test runs in.
fn git_evidence() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-evidence"));
    git_process::isolated(&mut command).env_remove("GIT_EVIDENCE_GIT_DIR");
    command
}

#[test]
fn a_bench_of_a_generated_store_times_every_verb_it_declares() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = git_evidence()
        .current_dir(dir.path())
        .env("TMPDIR", dir.path())
        .args([
            "bench",
            "--landings",
            "12",
            "--runs",
            "25",
            "--repeats",
            "2",
            "--csv",
        ])
        .output()
        .expect("bench");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("UTF-8");
    let mut lines = text.lines();
    assert_eq!(
        lines.next(),
        Some("verb,metric,value,unit,n,mad,outcome,detail")
    );
    // verb,metric,value,unit,n,mad,outcome,detail
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    let figure = |verb: &str, metric: &str| {
        rows.iter()
            .find(|r| r[0] == verb && r[1] == metric)
            .unwrap_or_else(|| panic!("no {metric} {verb} in {text}"))
    };
    assert_eq!(figure("overview", "cold_ms")[4], "1", "{text}");
    for verb in ["overview", "trend", "sql", "record"] {
        let warm = figure(verb, "warm_ms");
        assert_eq!((warm[4], warm[6]), ("2", "measured"), "{verb}: {text}");
        assert!(warm[2].parse::<f64>().expect("ms") > 0.0, "{verb}");
    }
    assert_eq!(rows.len(), 5, "one cold figure and four warm ones");
    // The bench removed its store.
    let left: Vec<_> = std::fs::read_dir(dir.path()).expect("dir").collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Benches a copy of `f`'s store, records the run it returns as `tool`,
/// and reads back each key and metric the run recorded.
fn benched(f: &Fixture) -> Vec<(String, String)> {
    let notes_before = f
        .git
        .run(&["for-each-ref", "refs/notes/"], None)
        .expect("refs");
    let index_before = std::fs::metadata(cache(f)).and_then(|m| m.modified()).ok();
    let returned = f.dir.path().join("tool-run.ndjson");
    let newest = f.landings.last().expect("a landing").as_str().to_owned();
    let output = git_evidence()
        .current_dir(f.dir.path())
        .env("GIT_EVIDENCE_GIT_DIR", f.git.git_dir())
        .args([
            "--trunk",
            "refs/heads/master",
            "bench",
            "--this-store",
            "--repeats",
            "1",
        ])
        .arg("--returned")
        .arg(&returned)
        .args(["--on", &newest])
        .output()
        .expect("bench");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The store benched was only read: no ref moved, and its index was not built.
    assert_eq!(
        f.git
            .run(&["for-each-ref", "refs/notes/"], None)
            .expect("refs"),
        notes_before
    );
    assert_eq!(
        std::fs::metadata(cache(f)).and_then(|m| m.modified()).ok(),
        index_before,
        "the bench wrote the store's own index"
    );
    let lines: Vec<git_evidence::EventLine> = std::fs::read_to_string(&returned)
        .expect("the returned run")
        .lines()
        .map(|line| serde_json::from_str(line).expect("an event line"))
        .collect();
    f.evidence
        .record(&kind("tool"), &lines)
        .expect("record tool");
    f.evidence
        .sql(
            "SELECT m.key, m.metric FROM measures m JOIN runs r USING (run)
             WHERE r.kind = 'tool' ORDER BY m.key, m.metric",
        )
        .expect("sql")
        .rows
        .iter()
        .map(|row| {
            let cell = |i: usize| row[i].cell().expect("a value");
            (cell(0), cell(1))
        })
        .collect()
}

#[test]
fn a_bench_of_this_store_records_each_verbs_figures_as_a_tool_run() {
    let f = fixture(2);
    f.git
        .run(&["config", "evidence.machine", "laptop"], None)
        .expect("machine");
    // Something for `trend` to follow.
    record_run(&f, &perf(), 1, &Shape::default(), &[("a", p99(10.0))], None);
    let pair = |key: &str, metric: &str| (key.to_owned(), metric.to_owned());
    assert_eq!(
        benched(&f),
        [
            pair("overview", "cold_ms"),
            pair("overview", "warm_ms"),
            pair("record", "warm_ms"),
            pair("sql", "warm_ms"),
            pair("store", "landings"),
            pair("store", "runs"),
            pair("trend", "warm_ms"),
        ]
    );
}
