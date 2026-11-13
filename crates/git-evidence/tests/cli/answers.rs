//! For every question, the default output is the Rust API's rows for the
//! same question as aligned columns under the same header, and the exit
//! status says what happened.

use crate::common::{Fixture, Shape, fixture, kind, newest, p99, perf, run_ids, tree};
use git_evidence::{
    At, DiffQuery, End, Error, Evidence, KeysQuery, LogQuery, ShowQuery, TidyRows, TrendQuery,
    Verdict, VerdictStatus,
};
use std::num::NonZeroU32;
use std::process::Command;

/// Four landings: perf measured on the first three, nothing on the newest;
/// a check that passes on landing 1 and fails on 2.
fn store() -> Fixture {
    let f = fixture(4);
    let mut perf_lines = Vec::new();
    for (landing, value) in [(0, 100.0), (1, 104.0), (2, 131.0)] {
        perf_lines.extend(f.run_lines(
            landing,
            &Shape::default(),
            &[("parse", p99(value))],
            Some(End::Complete),
        ));
    }
    f.evidence.record(&perf(), &perf_lines).expect("perf setup");
    let verdict = |status| {
        git_evidence::Outcome::Verdict(Verdict {
            status,
            tries: Vec::new(),
        })
    };
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

fn cli(f: &Fixture, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/master"])
        .args(args)
        .output()
        .expect("CLI")
}

/// Runs the command line and asserts it printed `expected`'s header and one
/// line per row, nothing on standard error, and exited 0.
fn prints<T: TidyRows>(f: &Fixture, args: &[&str], expected: &T) {
    let output = cli(f, args);
    let text = String::from_utf8(output.stdout.clone()).expect("UTF-8");
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().expect("a header").split_whitespace().collect();
    assert_eq!(header, expected.header(), "{args:?}");
    assert_eq!(lines.count(), expected.rows().len(), "{args:?}: {text}");
    assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
    assert_eq!(output.status.code(), Some(0), "{args:?}");
}

fn api<T>(f: &Fixture, question: impl FnOnce(&At<'_>) -> Result<T, Error>) -> T {
    Evidence::discover(f.dir.path())
        .expect("repository")
        .trunk("refs/heads/master")
        .at(question)
        .expect("ask")
}

fn ten() -> NonZeroU32 {
    NonZeroU32::new(10).expect("10")
}

fn the_overview_is_the_api_rows(f: &Fixture) {
    let overview = api(f, |at| at.overview(ten()));
    assert_eq!(overview.landings.len(), 4);
    prints(f, &[], &overview);
}

fn trend_is_the_api_rows(f: &Fixture) {
    let query = TrendQuery {
        metric: "block.p99_ns".parse().expect("name"),
        key: "parse".parse().expect("name"),
        machine: None,
        landings: newest(30),
    };
    prints(
        f,
        &["trend", "block.p99_ns", "parse"],
        &api(f, |at| at.trend(&query)),
    );
}

fn log_is_the_api_rows_one_line_per_run(f: &Fixture) {
    let landing = tree(f, &f.landings[1]);
    let runs = api(f, |at| at.log(&LogQuery::Tree(landing.clone())));
    assert_eq!(runs.len(), 2);
    prints(f, &["log", f.landings[1].as_str()], &runs);
    let text = String::from_utf8(cli(f, &["log", f.landings[1].as_str()]).stdout).expect("UTF-8");
    for run in &runs {
        let lines = text
            .lines()
            .filter(|l| l.contains(run.run.as_str()))
            .count();
        assert_eq!(lines, 1, "{text}");
    }
}

fn show_is_the_api_rows(f: &Fixture) {
    let run = run_ids(f, &perf()).remove(0);
    prints(f, &["show", run.as_str()], &api(f, |at| at.run(&run)));
    let landing = tree(f, &f.landings[2]);
    let runs = api(f, |at| at.show(&ShowQuery::tree(&landing)));
    assert_eq!(runs.len(), 2, "perf and check");
    prints(f, &["show", f.landings[2].as_str()], &runs);
    let verdicts = api(f, |at| {
        at.show(&ShowQuery {
            metric: Some("verdict".into()),
            ..ShowQuery::tree(&landing)
        })
    });
    prints(
        f,
        &["show", f.landings[2].as_str(), "--metric", "verdict"],
        &verdicts,
    );
    let text =
        String::from_utf8(cli(f, &["show", f.landings[2].as_str(), "--metric", "verdict"]).stdout)
            .expect("UTF-8");
    let row: Vec<&str> = text
        .lines()
        .nth(1)
        .expect("a row")
        .split_whitespace()
        .collect();
    assert_eq!(
        row[..5],
        ["check", "bench", "lint", "verdict", "fail"],
        "{text}"
    );
}

fn keys_is_the_api_rows(f: &Fixture) {
    prints(f, &["keys"], &api(f, |at| at.keys(&KeysQuery::default())));
}

fn diff_is_the_api_rows(f: &Fixture) {
    let query = DiffQuery {
        before: tree(f, &f.landings[0]),
        after: tree(f, &f.landings[1]),
        kind: None,
        key: None,
        metric: None,
        machine: None,
    };
    prints(
        f,
        &["diff", f.landings[0].as_str(), f.landings[1].as_str()],
        &api(f, |at| at.diff(&query)),
    );
}

fn no_matching_row_prints_the_header_alone_and_exits_0(f: &Fixture) {
    let output = cli(
        f,
        &["trend", "block.p99_ns", "parse", "--machine", "elsewhere"],
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).expect("UTF-8");
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.starts_with("landing"), "{text}");
}

fn a_name_nothing_records_exits_2_naming_it_on_standard_error(f: &Fixture) {
    let unknown_run = git_evidence::RunId::new();
    for (args, said) in [
        (vec!["trend", "block.p99_ns", "prase"], "nearest: parse"),
        (
            vec!["trend", "block.p9_ns", "parse"],
            "nearest: block.p99_ns",
        ),
        (
            vec!["keys", "--metric", "zzz*"],
            "no recorded metric matches",
        ),
        (
            vec!["keys", "--kind", "nosuch"],
            "no run of kind \"nosuch\"",
        ),
        (
            vec!["log", f.landings[3].as_str()],
            "nothing is recorded on tree",
        ),
        (vec!["show", unknown_run.as_str()], "is recorded"),
        (
            vec!["show", f.landings[3].as_str()],
            "nothing is recorded on tree",
        ),
        (
            vec!["show", f.landings[1].as_str(), "--key", "lnit"],
            "nearest: lint",
        ),
    ] {
        let output = cli(f, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8");
        assert!(stderr.contains(said), "{args:?}: {stderr}");
    }
}

fn the_overview_without_a_trunk_could_not_run(f: &Fixture) {
    let output = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/nothing"])
        .output()
        .expect("CLI");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--trunk"),
        "the fix is named: {output:?}"
    );
}

/// A malformed id on the command line is a malformed command (exit 1, before
/// any question), never a name nothing records (exit 2).
fn a_malformed_id_on_the_command_line_exits_1(f: &Fixture) {
    for args in [
        &["show", "not-a-ulid"][..],
        &["keys", "--kind", "Not A Kind"],
        &["trend", "latency.p99_ns", "", "--last", "2"],
        &["diff", "HEAD~1", "HEAD", "--machine", "two words"],
        &["nosuch", "HEAD~1", "HEAD"],
        &["--json", "keys"],
    ] {
        let output = cli(f, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}: nothing is printed");
    }
}

/// One owned history exercises every read-only command without rebuilding it.
#[test]
fn text_commands_preserve_rows_and_exit_statuses() {
    let f = store();
    the_overview_is_the_api_rows(&f);
    trend_is_the_api_rows(&f);
    log_is_the_api_rows_one_line_per_run(&f);
    show_is_the_api_rows(&f);
    keys_is_the_api_rows(&f);
    diff_is_the_api_rows(&f);
    no_matching_row_prints_the_header_alone_and_exits_0(&f);
    a_name_nothing_records_exits_2_naming_it_on_standard_error(&f);
    the_overview_without_a_trunk_could_not_run(&f);
    a_malformed_id_on_the_command_line_exits_1(&f);
}
