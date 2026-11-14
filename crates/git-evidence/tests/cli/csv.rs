//! `--csv` on every verb: its first line is the header the README documents
//! for the verb and the answer declares, and the rest is the Rust API's rows.

use crate::common::*;
use git_evidence::{
    At, DiffQuery, End, Error, Event, Evidence, KeysQuery, LogQuery, ShowQuery, TidyRows,
    TrendQuery, Verdict, VerdictStatus,
};
use std::num::NonZeroU32;
use std::process::Command;

/// Four landings: perf measured on the first three (the second under
/// disturbed conditions), nothing on the newest; a check that passes on
/// landing 1 and fails on 2.
fn store() -> Fixture {
    let f = fixture(4);
    let mut perf_lines = Vec::new();
    for (landing, value) in [(0, 100.0), (1, 104.0), (2, 131.0)] {
        let mut lines = f.run_lines(
            landing,
            &Shape::default(),
            &[("parse", p99(value))],
            Some(End::Complete),
        );
        if landing == 1
            && let Some(Event::Close { conditions, .. }) = lines.last_mut().map(|l| &mut l.event)
        {
            *conditions = Some(serde_json::json!({"disturbed": "load, high"}));
        }
        perf_lines.extend(lines);
    }
    f.evidence.record(&perf(), &perf_lines).expect("perf setup");
    let verdict = |status| git_evidence::Outcome::Verdict(Verdict::new(status, Vec::new()));
    let mut check_lines = Vec::new();
    for (landing, status) in [(1, VerdictStatus::Pass), (2, VerdictStatus::Fail)] {
        check_lines.extend(f.run_lines(
            landing,
            &Shape::default(),
            &[("lint", verdict(status))],
            Some(End::Complete),
        ));
    }
    f.evidence
        .record(&kind("check"), &check_lines)
        .expect("check setup");
    f
}

/// The header the README's Output table documents for `verb`.
fn documented(verb: &str) -> String {
    let readme = include_str!("../../README.md");
    let section = &readme[readme.find("## Output").expect("Output section")..];
    section
        .lines()
        .filter(|l| l.starts_with("| `"))
        .find_map(|row| {
            let cells: Vec<&str> = row.split('|').map(str::trim).collect();
            cells[1]
                .contains(&format!("`{verb}`"))
                .then(|| cells[2].trim_matches('`').to_owned())
        })
        .unwrap_or_else(|| panic!("the README documents no header for {verb}"))
}

/// The table as CSV: the reference the CLI's writer is compared with.
fn csv<T: TidyRows>(value: &T) -> String {
    let field = |cell: &str| {
        if cell.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", cell.replace('"', "\"\""))
        } else {
            cell.to_owned()
        }
    };
    let mut out = value.header().join(",") + "\n";
    for row in value.rows() {
        let cells: Vec<String> = row
            .iter()
            .map(|c| field(c.as_deref().unwrap_or_default()))
            .collect();
        out.push_str(&(cells.join(",") + "\n"));
    }
    out
}

fn cli(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--csv", "--trunk", "refs/heads/master"])
        .args(args)
        .output()
        .expect("CLI")
}

fn api<T>(f: &Fixture, question: impl FnOnce(&At<'_>) -> Result<T, Error>) -> T {
    Evidence::discover(f.dir.path())
        .expect("repository")
        .trunk("refs/heads/master")
        .at(question)
        .expect("ask")
}

/// `verb`'s CSV is its documented header, then the API's rows; nothing on
/// standard error, exit 0.
fn prints<T: TidyRows>(f: &Fixture, verb: &str, args: &[&str], value: &T) {
    let output = cli(f, args);
    let stdout = String::from_utf8(output.stdout.clone()).expect("UTF-8");
    // The overview's header names the kinds it found: the README writes them
    // `KIND…` and `KIND_refused…`.
    let header = value.header().join(",");
    let kinds = header
        .strip_prefix("landing,commit,date,")
        .and_then(|rest| rest.strip_suffix(",subject"));
    let header = match (verb, kinds) {
        ("git evidence", Some(kinds)) => header.replace(kinds, "KIND…,KIND_refused…"),
        _ => header,
    };
    assert_eq!(header, documented(verb), "{verb}");
    assert!(!value.rows().is_empty(), "{verb}: rows to compare");
    assert_eq!(stdout, csv(value), "{verb}");
    assert!(output.stderr.is_empty(), "{verb}: {output:?}");
    assert_eq!(output.status.code(), Some(0), "{verb}");
}

fn overview_csv(f: &Fixture) {
    let ten = NonZeroU32::new(10).expect("10");
    prints(f, "git evidence", &[], &api(f, |at| at.overview(ten)));
}

fn trend_csv_fills_every_point_column(f: &Fixture) {
    let query = TrendQuery::new(
        "block.p99_ns".parse().expect("name"),
        "parse".parse().expect("name"),
    )
    .landings(newest(30));
    let points = api(f, |at| at.trend(&query));
    prints(f, "trend", &["trend", "block.p99_ns", "parse"], &points);
    for row in points.rows() {
        assert!(row.iter().all(Option::is_some), "{row:?}");
    }
    assert_eq!(points.rows()[0][2].as_deref(), Some("2026-10-01T12:00:00Z"));
}

fn log_csv_names_each_run_landing(f: &Fixture) {
    let landing = tree(f, &f.landings[1]);
    prints(
        f,
        "log",
        &["log", f.landings[1].as_str()],
        &api(f, |at| at.log(&LogQuery::tree(&landing.clone()))),
    );
    let range = format!("{}..{}", f.landings[0], f.landings[2]);
    let output = cli(f, &["log", &range]);
    let landings: Vec<String> = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .skip(1)
        .map(|row| row.split(',').next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(landings, ["1", "1", "2", "2"]);
}

fn show_csv(f: &Fixture) {
    let run = run_ids(f, &perf()).remove(0);
    prints(
        f,
        "show",
        &["show", run.as_str()],
        &api(f, |at| at.run(&run)),
    );
    let landing = tree(f, &f.landings[1]);
    prints(
        f,
        "show",
        &["show", f.landings[1].as_str()],
        &api(f, |at| at.show(&ShowQuery::tree(&landing))),
    );
}

fn keys_csv(f: &Fixture) {
    prints(
        f,
        "keys",
        &["keys"],
        &api(f, |at| at.keys(&KeysQuery::default())),
    );
}

fn diff_csv_puts_before_and_after_side_by_side(f: &Fixture) {
    let query = DiffQuery::new(tree(f, &f.landings[0]), tree(f, &f.landings[1]));
    let rows = api(f, |at| at.diff(&query));
    prints(
        f,
        "diff",
        &["diff", f.landings[0].as_str(), f.landings[1].as_str()],
        &rows,
    );
    let text =
        String::from_utf8(cli(f, &["diff", f.landings[0].as_str(), f.landings[1].as_str()]).stdout)
            .expect("UTF-8");
    assert!(
        text.contains(",parse,block.p99_ns,100,104,+4.0%,ns,1,1,"),
        "{text}"
    );
}

fn audit_csv(f: &Fixture) {
    let output = cli(f, &["audit"]);
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    assert_eq!(stdout.lines().next(), Some(documented("audit").as_str()));
    let refs: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("ref,"))
        .map(|l| l.split(',').next().unwrap_or_default())
        .collect();
    assert_eq!(
        refs,
        ["refs/notes/evidence/check", "refs/notes/evidence/perf"]
    );
    assert_eq!(documented("push"), documented("fetch"));
    // The header itself is checked against `Transfer` in `share/remote.rs`.
}

#[test]
fn record_csv() {
    let f = store();
    let lines = f.run_lines(
        3,
        &Shape::default(),
        &[("parse", p99(1.0))],
        Some(End::Complete),
    );
    let input: String = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("JSON") + "\n")
        .collect();
    let mut child = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--csv", "record", "perf"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("CLI");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), input.as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("CLI");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8");
    let mut lines = stdout.lines();
    assert_eq!(lines.next(), Some(documented("record").as_str()));
    let row: Vec<&str> = lines.next().expect("a row").split(',').collect();
    assert_eq!(row[..3], [lines_run(&input).as_str(), "3", "0"]);
    // Delivered again: the same run, every line already present.
    let again = record(&f, &input);
    let row: Vec<&str> = again.lines().nth(1).expect("a row").split(',').collect();
    assert_eq!(row[..3], [lines_run(&input).as_str(), "0", "3"]);
}

/// An input holding no event line records nothing and says so by its exit
/// status, so a caller needs no reading of the table to know the store took
/// nothing.
#[test]
fn record_refuses_an_input_holding_no_event_line() {
    let f = store();
    for input in ["", "\n  \n"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
            .current_dir(f.dir.path())
            .args(["--csv", "record", "perf"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("CLI");
        std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), input.as_bytes())
            .expect("write");
        let output = child.wait_with_output().expect("CLI");
        assert_eq!(output.status.code(), Some(2), "{input:?}");
        assert!(output.stdout.is_empty(), "{input:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8");
        assert!(stderr.contains("no event line"), "{stderr}");
    }
}

fn record(f: &Fixture, input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--csv", "record", "perf"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("CLI");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), input.as_bytes())
        .expect("write");
    String::from_utf8(child.wait_with_output().expect("CLI").stdout).expect("UTF-8")
}

fn lines_run(input: &str) -> String {
    let first: serde_json::Value =
        serde_json::from_str(input.lines().next().expect("a line")).expect("JSON");
    first["run"].as_str().expect("run").to_owned()
}

fn sql_csv_header_is_the_query_columns(f: &Fixture) {
    let output = cli(
        f,
        &[
            "sql",
            "SELECT landing, subject FROM landings ORDER BY landing LIMIT 1",
        ],
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8"),
        "landing,subject\n0,landing 0\n"
    );
    assert_eq!(documented("sql"), "the query's own columns");
}

/// One owned history exercises every read-only command without rebuilding it.
#[test]
fn csv_commands_preserve_documented_columns_and_rows() {
    let f = store();
    overview_csv(&f);
    trend_csv_fills_every_point_column(&f);
    log_csv_names_each_run_landing(&f);
    show_csv(&f);
    keys_csv(&f);
    diff_csv_puts_before_and_after_side_by_side(&f);
    audit_csv(&f);
    sql_csv_header_is_the_query_columns(&f);
}
