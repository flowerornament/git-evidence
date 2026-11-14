//! One grammar for selecting landings: `--since` and `--until` select
//! exactly the landings dated inside the window, and `trend A..B` covers
//! exactly the landings `log A..B` lists. Also the landings the overview
//! lists: the pinned trunk, including unmeasured and empty commits.

mod common;

use common::*;
use git_evidence::{
    End, Error, LandingNumber, LandingRange, LogEntry, LogQuery, NotRecorded, Point, Selector,
    TidyRows, TrendQuery, UtcDate, testing::Scratch,
};
use std::collections::BTreeSet;
use std::num::NonZeroU32;
use std::process::Command;

/// Six landings dated 2026-10-01 to 10-06 (noon UTC), perf measured on all
/// but the newest.
fn store() -> Fixture {
    let f = fixture(6);
    for (landing, value) in [100.0, 104.0, 131.0, 99.0, 102.0].into_iter().enumerate() {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[("parse", p99(value))],
            Some(End::Complete),
        );
    }
    f
}

fn date(text: &str) -> UtcDate {
    text.parse().expect("date")
}

fn range(f: &Fixture, after: usize, through: usize) -> Selector {
    Selector::default().range(LandingRange {
        after: f.landings[after].clone(),
        through: f.landings[through].clone(),
    })
}

fn log(f: &Fixture, selector: &Selector) -> Vec<LogEntry> {
    ask(f, |at| at.log(&LogQuery::landings(selector.clone())))
}

/// The landings a log lists.
fn listed(entries: &[LogEntry]) -> BTreeSet<u32> {
    entries
        .iter()
        .filter_map(|e| e.landing.map(LandingNumber::get))
        .collect()
}

fn trend(f: &Fixture, landings: Selector) -> Vec<Point> {
    let query = TrendQuery::new(
        "block.p99_ns".parse().expect("name"),
        "parse".parse().expect("name"),
    )
    .landings(landings);
    ask(f, |at| at.trend(&query))
}

fn points(points: &[Point]) -> BTreeSet<u32> {
    points.iter().map(|p| p.landing.get()).collect()
}

#[test]
fn since_and_until_select_exactly_the_landings_dated_inside() {
    let f = store();
    let window = |since: Option<&str>, until: Option<&str>| {
        let mut selector = Selector::default();
        (selector.since, selector.until) = (since.map(date), until.map(date));
        selector
    };
    // Whole days: the until day is included.
    let days = log(&f, &window(Some("2026-10-02"), Some("2026-10-04")));
    assert_eq!(listed(&days), BTreeSet::from([1, 2, 3]));
    // Seconds: landing 1 is dated 12:00:00 exactly, so one second later leaves it out.
    let seconds = log(
        &f,
        &window(Some("2026-10-02T12:00:01Z"), Some("2026-10-04T12:00:00Z")),
    );
    assert_eq!(listed(&seconds), BTreeSet::from([2, 3]));
    // --last narrows what the window leaves.
    let last = log(&f, &{
        let mut q = window(Some("2026-10-02"), Some("2026-10-05"));
        q.last = std::num::NonZeroU32::new(2);
        q
    });
    assert_eq!(listed(&last), BTreeSet::from([3, 4]));
    assert_eq!(
        points(&trend(&f, window(Some("2026-10-05"), None))),
        BTreeSet::from([4])
    );
}

#[test]
fn trend_over_a_range_covers_exactly_the_landings_log_lists() {
    let f = store();
    let selector = range(&f, 0, 3);
    let log = log(&f, &selector);
    let trend = trend(&f, selector);
    assert_eq!(listed(&log), BTreeSet::from([1, 2, 3]));
    assert_eq!(points(&trend), listed(&log));
}

#[test]
fn a_range_end_off_the_trunk_is_named_and_an_empty_window_is_empty() {
    let f = store();
    let candidate = f
        .git
        .write_commit(
            &tree(&f, &f.landings[2]),
            &[f.landings[1].object()],
            "candidate",
        )
        .expect("commit");
    let off_trunk = Selector::default().range(LandingRange {
        after: f.landings[0].clone(),
        through: candidate,
    });
    let missing = not_recorded(&f, |at| at.log(&LogQuery::landings(off_trunk.clone())));
    assert!(
        matches!(missing, NotRecorded::NotALanding(_)),
        "{missing:?}"
    );
    let future = Selector::default().since(date("2027-01-01"));
    assert_eq!(log(&f, &future), Vec::new());
}

#[test]
fn trend_prints_one_row_per_run_and_nothing_else() {
    let f = store();
    let text = cli(&f, &["trend", "block.p99_ns", "parse"]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "a header and five rows: {text}");
    assert!(
        lines[0].starts_with("landing  commit"),
        "the header: {text}"
    );
    assert!(lines[1].contains(" 100  ns "), "{text}");
    assert!(lines[5].contains(" 102  ns "), "{text}");
}

fn cli(f: &Fixture, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/master"])
        .args(args)
        .output()
        .expect("CLI");
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn the_command_line_takes_the_grammar_on_trend_and_log() {
    let f = store();
    let a_b = format!("{}..{}", f.landings[0], f.landings[3]);
    assert_eq!(
        cli(&f, &["--csv", "log", &a_b]),
        csv_of(&log(&f, &range(&f, 0, 3)))
    );
    assert_eq!(
        cli(&f, &["--csv", "trend", "block.p99_ns", "parse", &a_b]),
        csv_of(&trend(&f, range(&f, 0, 3)))
    );
    let dated = Selector::default().since(date("2026-10-04"));
    assert_eq!(
        cli(&f, &["--csv", "log", "--since", "2026-10-04"]),
        csv_of(&log(&f, &dated))
    );
    let refused = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args([
            "--trunk",
            "refs/heads/master",
            "log",
            f.landings[1].as_str(),
            "--last",
            "2",
        ])
        .output()
        .expect("CLI");
    assert_eq!(refused.status.code(), Some(1));
    let bad_date = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args([
            "--trunk",
            "refs/heads/master",
            "log",
            "--since",
            "yesterday",
        ])
        .output()
        .expect("CLI");
    assert_eq!(bad_date.status.code(), Some(1));
}

/// `--last` counts landings from one: no count reads as "no limit". Zero or a
/// negative count is refused. `trend` without a selection reads its own
/// default and no Git config for one.
#[test]
fn last_counts_from_one() {
    let f = store();
    for bad in ["0", "-1"] {
        let output = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
            .current_dir(f.dir.path())
            .args(["--trunk", "refs/heads/master", "log", "--last", bad])
            .output()
            .expect("CLI");
        assert_eq!(output.status.code(), Some(1), "--last {bad}");
        assert!(output.stdout.is_empty(), "--last {bad}");
    }
    let horizon = Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/master", "log", "--horizon", "1"])
        .output()
        .expect("CLI");
    assert_eq!(horizon.status.code(), Some(1), "--horizon is retired");
    f.git
        .run(&["config", "evidence.horizon", "1"], None)
        .expect("config");
    let query = TrendQuery::new(
        "block.p99_ns".parse().expect("metric"),
        "parse".parse().expect("key"),
    )
    .landings({
        let mut q = Selector::default();
        q.last = TrendQuery::DEFAULT_LAST.into();
        q
    });
    assert_eq!(
        cli(&f, &["--csv", "trend", "block.p99_ns", "parse"]),
        csv_of(&ask(&f, |at| at.trend(&query)))
    );
}

/// Rows as `--csv` prints them; these rows hold no comma, quote or break.
fn csv_of<T: TidyRows>(value: &T) -> String {
    std::iter::once(value.header())
        .chain(
            value
                .rows()
                .into_iter()
                .map(|row| row.into_iter().map(Option::unwrap_or_default).collect()),
        )
        .map(|cells| cells.join(",") + "\n")
        .collect()
}

#[test]
fn landings_are_the_newest_oldest_first_and_include_empty_unmeasured_commits() {
    let scratch = Scratch::new()
        .expect("scratch")
        .landings(3)
        .expect("landings");
    for args in [
        vec!["read-tree", "HEAD"],
        vec!["commit", "--allow-empty", "-m", "empty landing"],
    ] {
        let output = scratch.command("git").args(args).output().expect("git");
        assert!(output.status.success(), "{output:?}");
    }
    let evidence = scratch.evidence();
    let last = |n| NonZeroU32::new(n).expect("a count above zero");
    let (all, two) = evidence
        .at(|at| Ok((at.overview(last(99))?, at.overview(last(2))?)))
        .expect("one pin");
    let rows: Vec<_> = all.landings.iter().map(|l| &l.landing).collect();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3].subject, "empty landing");
    assert_eq!(rows[3].tree, rows[2].tree);
    assert_eq!(rows[2].commit, scratch.landing(2));
    assert!(
        rows.windows(2)
            .all(|w| w[0].number.get() + 1 == w[1].number.get())
    );
    assert_eq!(two.landings, all.landings[2..]);
    assert!(all.landings.iter().all(|l| l.runs.is_empty()));
}

#[test]
fn an_unresolved_trunk_is_an_error_never_an_empty_list() {
    let scratch = Scratch::new().expect("scratch");
    let answer = scratch
        .evidence()
        .at(|at| at.overview(NonZeroU32::new(30).expect("30")));
    assert!(
        matches!(answer, Err(Error::Store(git_evidence::StoreError::NoTrunk))),
        "{answer:?}"
    );
}

#[test]
fn invalid_recorded_runs_are_no_runs_and_leave_the_landings_alone() {
    let scratch = Scratch::new()
        .expect("scratch")
        .landings(1)
        .expect("landing")
        .machine("laptop")
        .expect("declared machine");
    let evidence = scratch.evidence();
    let commit = scratch.landing(0);
    let kind: git_evidence::Kind = "gates".parse().expect("kind");
    let open = git_evidence::Open::new(commit, "population-test".parse().expect("producer"))
        .harness("population-test/1")
        .machine(evidence.machine().expect("machine"));
    let _run = evidence.run(&kind, open).expect("open run");
    let one = NonZeroU32::new(1).expect("1");
    let population = evidence.at(|at| at.overview(one)).expect("population");
    let tree = population.landings[0].landing.tree.clone();
    assert_eq!(population.landings[0].runs["gates"].runs, 1);
    let raw = git_evidence::testing::note(&evidence, &kind, &tree)
        .expect("note read")
        .expect("recorded note");
    let mut line: serde_json::Value = serde_json::from_slice(&raw).expect("open line");
    line["v"] = serde_json::json!(1);
    let output = scratch
        .command("git")
        .args([
            "notes",
            "--ref=refs/notes/evidence/gates",
            "add",
            "-f",
            "-m",
        ])
        .arg(line.to_string())
        .arg(tree.as_str())
        .output()
        .expect("replace note");
    assert!(output.status.success(), "{output:?}");
    let (population, log) = evidence
        .at(|at| {
            Ok((
                at.overview(one)?,
                at.log(&LogQuery::tree(&tree.clone()))
                    .map_err(|e| e.to_string()),
            ))
        })
        .expect("same pin");
    assert_eq!(population.landings.len(), 1);
    assert!(population.landings[0].runs.is_empty(), "{population:?}");
    assert!(
        log.as_ref()
            .is_err_and(|e| e.contains("nothing is recorded")),
        "{log:?}"
    );
    let audit = evidence.audit().expect("audit");
    assert!(!audit.findings.is_empty(), "the unreadable line is named");
}
