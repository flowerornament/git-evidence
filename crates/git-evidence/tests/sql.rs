//! `sql`: each public view answers on a fixture store, a statement that
//! writes is refused, a note recorded after the last refresh is in the next
//! answer, and the command line prints the API's rows as CSV and nothing else.

mod common;

use common::*;
use git_evidence::{
    End, Error, Event, IndexError, Outcome, PUBLIC_SCHEMA, Rows, SqlError, Try, Verdict,
    VerdictStatus,
};
use std::process::Command;

/// Four landings: perf measured complete on 0, under disturbed conditions
/// on 1, refused on 2, and open on 3; on landing 2 a gates run whose check
/// passed and lint failed.
fn store() -> Fixture {
    let f = fixture(4);
    let shape = Shape {
        params: vec![("suite", "full")],
        ..Shape::default()
    };
    let both = |ours, theirs| {
        Outcome::Measured(vec![
            git_evidence::Measure::new("block.p99_ns".parse().expect("metric"), ours)
                .expect("finite"),
            git_evidence::Measure::new("ref.p99_ns".parse().expect("metric"), theirs)
                .expect("finite"),
        ])
    };
    record_run(
        &f,
        &perf(),
        0,
        &shape,
        &[("parse", both(100.0, 50.0))],
        Some(End::Complete),
    );
    let mut disturbed = f.run_lines(1, &shape, &[("parse", p99(104.0))], Some(End::Complete));
    if let Some(Event::Close { conditions, .. }) = disturbed.last_mut().map(|l| &mut l.event) {
        *conditions = Some(serde_json::json!({"disturbed": "load 9"}));
    }
    f.evidence
        .record(&kind("perf"), &disturbed)
        .expect("record");
    record_run(
        &f,
        &perf(),
        2,
        &shape,
        &[("parse", refused("noisy_host"))],
        Some(End::Complete),
    );
    record_run(&f, &perf(), 3, &shape, &[("parse", p99(99.0))], None);
    let verdict = |status, duration_ms, cpu_ms| {
        Outcome::Verdict(Verdict::new(
            status,
            vec![{
                let mut t = Try::new(status);
                (t.duration_ms, t.cpu_ms) = (duration_ms, cpu_ms);
                t
            }],
        ))
    };
    record_run(
        &f,
        &kind("gates"),
        2,
        &Shape {
            producer: "gates",
            ..Shape::default()
        },
        &[
            (
                "check-facts",
                verdict(VerdictStatus::Pass, Some(12.0), Some(30.0)),
            ),
            ("lint", verdict(VerdictStatus::Fail, None, None)),
        ],
        Some(End::Complete),
    );
    f
}

fn rows(f: &Fixture, query: &str) -> Vec<Vec<String>> {
    f.evidence
        .sql(query)
        .expect("sql")
        .rows
        .iter()
        .map(|row| row.iter().map(|v| v.cell().unwrap_or_default()).collect())
        .collect()
}

fn table(cells: &[&[&str]]) -> Vec<Vec<String>> {
    cells
        .iter()
        .map(|row| row.iter().map(|c| (*c).to_owned()).collect())
        .collect()
}

#[test]
fn about_names_the_schema_and_the_pinned_trunk() {
    let f = store();
    let views = f.evidence.at(|at| Ok(at.pin().views_version)).expect("pin");
    assert_eq!(
        rows(
            &f,
            "SELECT schema_version, views_version, trunk_ref, trunk, pending_views FROM about"
        ),
        table(&[&[
            &PUBLIC_SCHEMA.to_string(),
            &views.to_string(),
            "refs/heads/master",
            f.landings[3].as_str(),
            "",
        ]])
    );
    assert_eq!(
        rows(&f, "PRAGMA user_version"),
        table(&[&[&PUBLIC_SCHEMA.to_string()]])
    );
}

#[test]
fn landings_are_the_trunk_with_utc_dates() {
    let f = store();
    assert_eq!(
        rows(
            &f,
            "SELECT landing, commit_id, date, subject FROM landings ORDER BY landing"
        ),
        (0..4)
            .map(|n| vec![
                n.to_string(),
                f.landings[n].to_string(),
                format!("2026-10-0{}T12:00:00Z", n + 1),
                format!("landing {n}"),
            ])
            .collect::<Vec<_>>()
    );
}

#[test]
fn runs_carry_conditions_and_how_they_ended() {
    let f = store();
    assert_eq!(
        rows(
            &f,
            "SELECT kind, landing, conditions, disturbed, ended, closed IS NULL
             FROM runs ORDER BY landing, kind"
        ),
        table(&[
            &["perf", "0", "quiet", "", "complete", "0"],
            &["perf", "1", "disturbed", "load 9", "complete", "0"],
            &["gates", "2", "quiet", "", "complete", "0"],
            &["perf", "2", "quiet", "", "complete", "0"],
            &["perf", "3", "quiet", "", "", "1"],
        ])
    );
    assert_eq!(
        rows(&f, "SELECT DISTINCT name, value FROM params"),
        table(&[&["suite", "full"]])
    );
}

/// A run's close conditions are rows of `conditions`, one per scalar by its
/// dotted path, and they are exactly the `condition.PATH` rows `show RUN`
/// prints: one flattening, two readers.
#[test]
fn close_conditions_are_rows_and_show_prints_the_same_ones() {
    let f = store();
    let mut lines = f.run_lines(
        2,
        &Shape::default(),
        &[("land", p99(7.0))],
        Some(End::Complete),
    );
    let id = lines[0].run.clone();
    if let Some(Event::Close { conditions, .. }) = lines.last_mut().map(|l| &mut l.event) {
        *conditions = Some(serde_json::json!({
            "cause": {"class": "build", "row": "compile"},
            "queue_wait_ms": 12.5,
            "findings": [{"issue": "issue-42"}],
            "none": null,
            "empty": {},
            "a.b": 1,
            "a": {"b": 2},
        }));
    }
    f.evidence.record(&kind("gates"), &lines).expect("record");
    // `a.b` and `a` holding `b` spell one path: both rows stay, as `show`
    // prints both.
    let expected = [
        ["a.b", "1"],
        ["a.b", "2"],
        ["cause.class", "build"],
        ["cause.row", "compile"],
        ["empty", "{}"],
        ["findings.0.issue", "issue-42"],
        ["none", ""],
        ["queue_wait_ms", "12.5"],
    ];
    assert_eq!(
        rows(
            &f,
            &format!(
                "SELECT path, value FROM conditions WHERE run = '{}' ORDER BY path, value",
                id.as_str()
            )
        ),
        expected
            .iter()
            .map(|row| row.map(str::to_owned).to_vec())
            .collect::<Vec<_>>()
    );
    let detail = f.evidence.at(|at| at.run(&id)).expect("show");
    let mut shown: Vec<Vec<String>> = git_evidence::TidyRows::rows(&detail)
        .into_iter()
        .filter_map(|row| {
            let path = row[3].as_deref()?.strip_prefix("condition.")?.to_owned();
            Some(vec![path, row[4].clone().unwrap_or_default()])
        })
        .collect();
    shown.sort();
    assert_eq!(
        shown,
        expected
            .iter()
            .map(|row| row.map(str::to_owned).to_vec())
            .collect::<Vec<_>>()
    );
    // A JSON null is no value (NULL), not empty text.
    assert_eq!(
        rows(
            &f,
            &format!(
                "SELECT value IS NULL FROM conditions WHERE run = '{}' AND path = 'none'",
                id.as_str()
            )
        ),
        table(&[&["1"]])
    );
    // Every other run of the store: only the disturbed one closed with
    // conditions; a run without them has no rows.
    assert_eq!(
        rows(
            &f,
            &format!(
                "SELECT path, value FROM conditions WHERE run != '{}'",
                id.as_str()
            )
        ),
        table(&[&["disturbed", "load 9"]])
    );
}

#[test]
fn measures_verdicts_and_outcomes_keep_every_outcome() {
    let f = store();
    assert_eq!(
        rows(
            &f,
            "SELECT r.landing, m.key, m.metric, m.value, m.unit, m.direction, m.origin
             FROM measures m JOIN runs r USING (run) WHERE r.kind = 'perf'
             ORDER BY r.landing, m.metric"
        ),
        table(&[
            &[
                "0",
                "parse",
                "block.p99_ns",
                "100",
                "ns",
                "lower",
                "recorded"
            ],
            &["0", "parse", "ref.p99_ns", "50", "ns", "lower", "recorded"],
            &[
                "1",
                "parse",
                "block.p99_ns",
                "104",
                "ns",
                "lower",
                "recorded"
            ],
            &[
                "3",
                "parse",
                "block.p99_ns",
                "99",
                "ns",
                "lower",
                "recorded"
            ],
        ])
    );
    assert_eq!(
        rows(
            &f,
            "SELECT key, status, duration_ms, cpu_ms, tries FROM verdicts ORDER BY key"
        ),
        table(&[
            &["check-facts", "pass", "12", "30", "1"],
            &["lint", "fail", "", "", "1"],
        ])
    );
    assert_eq!(
        rows(
            &f,
            "SELECT o.key, o.outcome, o.reason FROM outcomes o JOIN runs r USING (run)
             WHERE r.landing = 2 ORDER BY o.key"
        ),
        table(&[
            &["check-facts", "verdict", ""],
            &["lint", "verdict", ""],
            &["parse", "refused", "noisy_host"],
        ])
    );
}

#[test]
fn a_statement_that_writes_or_returns_no_rows_is_refused() {
    let f = store();
    for statement in [
        "INSERT INTO pin (ref, tip) VALUES ('x', 'y')",
        "DELETE FROM all_runs",
        "UPDATE meta SET value = '0'",
        "CREATE TABLE t (x)",
        "DROP VIEW runs",
    ] {
        assert!(
            matches!(f.evidence.sql(statement), Err(Error::Sql(SqlError::Writes))),
            "{statement}"
        );
    }
    for statement in ["BEGIN", "COMMIT", "ATTACH ':memory:' AS other"] {
        assert!(
            matches!(f.evidence.sql(statement), Err(Error::Sql(SqlError::NoRows))),
            "{statement}"
        );
    }
    assert_eq!(rows(&f, "SELECT count(*) FROM all_runs"), table(&[&["5"]]));
}

#[test]
fn duplicate_column_names_are_refused() {
    let f = store();
    assert!(matches!(
        f.evidence.sql("SELECT r.run, m.run FROM runs r JOIN measures m USING (run)"),
        Err(Error::Sql(SqlError::DuplicateColumn(name))) if name == "run"
    ));
}

#[test]
fn a_note_recorded_after_the_last_refresh_is_in_the_next_answer() {
    let f = store();
    let count = "SELECT count(*) FROM measures WHERE value = 77";
    assert_eq!(rows(&f, count), table(&[&["0"]]));
    record_run(
        &f,
        &perf(),
        3,
        &Shape::default(),
        &[("parse", p99(77.0))],
        Some(End::Complete),
    );
    assert_eq!(rows(&f, count), table(&[&["1"]]));
}

fn cli(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/master", "sql"])
        .args(args)
        .output()
        .expect("CLI")
}

#[test]
fn the_command_line_prints_the_api_rows() {
    let f = store();
    let query = "SELECT landing, subject, NULL AS absent FROM landings ORDER BY landing LIMIT 2";
    let rows: Rows = f.evidence.sql(query).expect("sql");
    assert_eq!(
        rows.rows[1],
        [
            git_evidence::SqlValue::Integer(1),
            git_evidence::SqlValue::Text("landing 1".into()),
            git_evidence::SqlValue::Null
        ]
    );
    let csv = cli(&f, &[query]);
    assert!(csv.status.success(), "{csv:?}");
    assert_eq!(
        String::from_utf8(csv.stdout).expect("UTF-8"),
        "landing,subject,absent\n0,landing 0,\n1,landing 1,\n"
    );
    assert!(csv.stderr.is_empty(), "nothing on standard error");
    let refused = cli(&f, &["DELETE FROM all_runs"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
}

/// The README's examples open with a `--` comment line; pasted as the query
/// they are the query, not an option.
#[test]
fn a_query_that_opens_with_a_comment_is_the_query() {
    let f = store();
    let output = cli(
        &f,
        &["-- the first landing\nSELECT landing FROM landings ORDER BY landing LIMIT 1"],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8"),
        "landing\n0\n"
    );
    let csv = cli(&f, &["-- one\nSELECT 1 AS one", "--csv"]);
    assert!(csv.status.success(), "{csv:?}");
    assert_eq!(String::from_utf8(csv.stdout).expect("UTF-8"), "one\n1\n");
    // A flag in the query's place is still the flag.
    let path = cli(&f, &["--path"]);
    assert!(path.status.success(), "{path:?}");
}

#[test]
fn path_refreshes_and_names_the_index_file() {
    let f = store();
    let output = cli(&f, &["--path"]);
    assert!(output.status.success(), "{output:?}");
    let path = String::from_utf8(output.stdout).expect("UTF-8");
    let path = std::path::Path::new(path.trim());
    let canonical = |p: &std::path::Path| p.canonicalize().expect("exists");
    assert_eq!(
        canonical(path),
        canonical(&f.evidence.sql_path().expect("path"))
    );
    assert!(path.is_file(), "{}", path.display());
}

/// Records one run of `kind` on landing 0 that names `source`.
fn sourced(f: &Fixture, kind: &str, source: &str, result: (&str, Outcome)) {
    let mut lines = f.run_lines(0, &Shape::default(), &[result], Some(End::Complete));
    if let Event::Open(open) = &mut lines[0].event {
        open.source = Some(source.into());
    }
    f.evidence
        .record(&self::kind(kind), &lines)
        .expect("record");
}

/// A build whose fold differs, with the same views version, leaves rows of its
/// own folding; this build answers from its own fold of the notes.
#[test]
fn another_builds_folded_rows_are_replaced_before_an_answer() {
    let f = fixture(1);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse", p99(100.0))],
        Some(End::Complete),
    );
    let answer = "SELECT l.subject, m.value FROM measures m JOIN runs r USING (run)
                  JOIN landings l ON l.tree = r.tree";
    assert_eq!(rows(&f, answer), table(&[&["landing 0", "100"]]));
    let db = rusqlite::Connection::open(cache(&f)).expect("index");
    db.execute_batch(
        "UPDATE all_measures SET value = 999;
         UPDATE trunk SET subject = 'folded by another build';
         UPDATE meta SET value = 'sha256:another-build' WHERE key = 'source';",
    )
    .expect("another build's folding");
    drop(db);
    assert_eq!(
        rows(&f, answer),
        table(&[&["landing 0", "100"]]),
        "every table folded again by this build"
    );
}

#[test]
fn another_builds_views_are_replaced_before_an_answer() {
    let f = fixture(1);
    sourced(&f, "perf", "run-1.json", ("parse", p99(1.0)));
    let answer = "SELECT DISTINCT origin FROM measures";
    assert_eq!(rows(&f, answer), table(&[&["recorded"]]));
    // What a binary of the same views version but other source leaves: an
    // older `measures` without `origin`.
    let db = rusqlite::Connection::open(cache(&f)).expect("index");
    db.execute_batch(
        "DROP VIEW measures;
         CREATE VIEW measures AS SELECT run, key, metric, value FROM all_measures;
         UPDATE meta SET value = 'sha256:another-build' WHERE key = 'source';",
    )
    .expect("another build's preparation");
    drop(db);
    assert_eq!(rows(&f, answer), table(&[&["recorded"]]), "views recreated");
}

/// A query that runs past its time limit is stopped and refused, naming
/// the limit, from the API and the command line; without a limit it would
/// run for hours.
#[test]
fn a_query_past_its_time_limit_is_stopped_and_refused() {
    let f = store();
    let slow = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c \
                WHERE x < 10000000000) SELECT max(x) FROM c";
    let started = std::time::Instant::now();
    let refused = f
        .evidence
        .clone()
        .time_limit(std::time::Duration::from_millis(300))
        .sql(slow);
    assert!(
        matches!(
            refused,
            Err(Error::Sql(SqlError::Index(IndexError::TimeLimit { .. })))
        ),
        "{refused:?}"
    );
    // The query runs for hours: returning inside the hang guard stopped it.
    assert!(started.elapsed() < git_evidence::testing::HANG_GUARD);
    let output = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args([
            "--trunk",
            "refs/heads/master",
            "--time-limit",
            "1",
            "sql",
            slow,
        ])
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8");
    assert!(stderr.contains("time limit of 1 s"), "{stderr}");
}
