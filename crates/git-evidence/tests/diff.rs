//! `diff A B`: the numbers that differ between two trees, each beside its
//! counterpart, matched on kind, machine, variant, key, metric and unit. Each
//! row names its variant; the newest complete run of a series gives its
//! number, and the row counts the runs it chose from. A number on one side
//! only keeps the other side empty. Equal numbers are no rows.

mod common;

use common::*;
use git_evidence::{Change, DiffQuery, Difference, End, NotRecorded, Reason};

fn between(f: &Fixture, before: usize, after: usize) -> DiffQuery {
    DiffQuery::new(tree(f, &f.landings[before]), tree(f, &f.landings[after]))
}

/// Each row as (key, metric, before, after).
fn sides(rows: &[Difference]) -> Vec<(String, String, Option<f64>, Option<f64>)> {
    rows.iter()
        .map(|r| {
            (
                r.key.clone(),
                r.metric.clone(),
                r.before.as_ref().map(|v| v.value),
                r.after.as_ref().map(|v| v.value),
            )
        })
        .collect()
}

#[test]
fn numbers_on_both_trees_sit_side_by_side_and_one_sided_ones_keep_an_empty_side() {
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("chain", p99(100.0)), ("gone", p99(5.0))],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("chain", p99(110.0)), ("new", p99(7.0))],
        Some(End::Complete),
    );
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    let p99 = "block.p99_ns".to_owned();
    assert_eq!(
        sides(&rows),
        vec![
            ("chain".into(), p99.clone(), Some(100.0), Some(110.0)),
            ("gone".into(), p99.clone(), Some(5.0), None),
            ("new".into(), p99, None, Some(7.0)),
        ]
    );
    assert_eq!(rows[0].change(), Some(Change::Relative(0.1)));
    assert_eq!(rows[1].change(), None, "no change without both sides");
}

#[test]
fn the_newest_complete_run_gives_a_number_and_the_row_names_it() {
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("chain", p99(100.0))],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("chain", p99(120.0))],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("chain", p99(130.0))],
        Some(End::Complete),
    );
    // Neither a refused nor an open run gives a number, however new.
    let refused = End::Refused(Reason::new("noisy_host"));
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("chain", p99(999.0))],
        Some(refused),
    );
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("chain", p99(998.0))],
        None,
    );
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    assert_eq!(
        sides(&rows),
        vec![(
            "chain".into(),
            "block.p99_ns".into(),
            Some(100.0),
            Some(130.0)
        )]
    );
    let newest = run_ids(&f, &perf())[2].clone();
    assert_eq!(rows[0].after.as_ref().map(|v| &v.run), Some(&newest));
    let runs = |side: &Option<git_evidence::RunValue>| side.as_ref().map(|v| v.runs);
    assert_eq!(
        (runs(&rows[0].before), runs(&rows[0].after)),
        (Some(1), Some(2)),
        "two complete runs on the later tree, and the row says so"
    );
    let text = csv(&f, 0, 1);
    assert!(
        text.lines()
            .nth(1)
            .is_some_and(|row| row.contains(",ns,1,2,")),
        "{text}"
    );
}

/// `diff A B --csv` for landings `a` and `b`.
fn csv(f: &Fixture, a: usize, b: usize) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--csv", "--trunk", "refs/heads/master", "diff"])
        .args([f.landings[a].as_str(), f.landings[b].as_str()])
        .output()
        .expect("CLI");
    String::from_utf8(output.stdout).expect("UTF-8")
}

#[test]
fn numbers_of_different_variants_are_never_matched() {
    let f = fixture(2);
    let other = Shape {
        harness: "h/2",
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("chain", p99(100.0))],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &other,
        &[("chain", p99(50.0))],
        Some(End::Complete),
    );
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    assert_eq!(
        sides(&rows),
        vec![
            ("chain".into(), "block.p99_ns".into(), Some(100.0), None),
            ("chain".into(), "block.p99_ns".into(), None, Some(50.0)),
        ]
        .into_iter()
        .filter(|row| sides(&rows).contains(row))
        .collect::<Vec<_>>()
    );
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r.change().is_none()));
    // Two series of one key, metric and machine: their rows name different variants.
    assert_ne!(rows[0].short_variant(), rows[1].short_variant());
    let text = csv(&f, 0, 1);
    let variants: Vec<&str> = text
        .lines()
        .skip(1)
        .map(|row| row.split(',').nth(2).unwrap_or_default())
        .collect();
    assert_eq!(variants.len(), 2, "{text}");
    assert!(variants.iter().all(|v| v.len() == 8), "{text}");
    assert_ne!(variants[0], variants[1], "{text}");
}

#[test]
fn a_number_in_another_unit_is_another_row_never_compared() {
    let f = fixture(2);
    let micros = Shape {
        unit: "us",
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("chain", p99(100.0))],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &micros,
        &[("chain", p99(0.1))],
        Some(End::Complete),
    );
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    let units: Vec<(Option<f64>, Option<f64>, &str)> = rows
        .iter()
        .map(|r| {
            let unit = r
                .before
                .as_ref()
                .or(r.after.as_ref())
                .map_or("", |v| v.unit.as_str());
            (
                r.before.as_ref().map(|v| v.value),
                r.after.as_ref().map(|v| v.value),
                unit,
            )
        })
        .collect();
    assert_eq!(
        units,
        vec![(Some(100.0), None, "ns"), (None, Some(0.1), "us")]
    );
    assert!(rows.iter().all(|r| r.change().is_none()));
}

#[test]
fn a_change_is_relative_unless_before_is_zero_or_the_unit_is_a_ratio() {
    let f = fixture(2);
    let ratio = Shape {
        unit: "1",
        ..Shape::default()
    };
    for (landing, zero, share) in [(0, 0.0, 0.5), (1, 3.0, 0.75)] {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[("zero", p99(zero))],
            Some(End::Complete),
        );
        record_run(
            &f,
            &kind("load"),
            landing,
            &ratio,
            &[("share", p99(share))],
            Some(End::Complete),
        );
    }
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    let change = |key: &str| {
        rows.iter()
            .find(|r| r.key == key)
            .and_then(Difference::change)
    };
    assert_eq!(change("zero"), Some(Change::Absolute(3.0)));
    assert_eq!(change("share"), Some(Change::Absolute(0.25)));
}

#[test]
fn filters_narrow_the_rows() {
    let f = fixture(2);
    let laptop = Shape {
        machine: "laptop",
        ..Shape::default()
    };
    for (landing, step) in [(0, 0.0), (1, 1.0)] {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[
                ("chain", p99(1.0 + step)),
                ("swap", measured("compile.us", 2.0 + step)),
            ],
            Some(End::Complete),
        );
        record_run(
            &f,
            &perf(),
            landing,
            &laptop,
            &[("chain", p99(3.0 + step))],
            Some(End::Complete),
        );
    }
    let asked = |query: DiffQuery| ask(&f, |at| at.diff(&query)).len();
    assert_eq!(asked(between(&f, 0, 1)), 3);
    assert_eq!(
        asked(between(&f, 0, 1).machine("laptop".parse().expect("machine"))),
        1
    );
    assert_eq!(asked(between(&f, 0, 1).metric("compile.*")), 1);
    assert_eq!(
        asked(between(&f, 0, 1).key("chain".parse().expect("key"))),
        2
    );
    assert_eq!(asked(between(&f, 0, 1).kind(kind("gates"))), 0);
}

#[test]
fn a_tree_with_no_run_an_unknown_key_or_an_unmatched_glob_is_not_recorded() {
    let f = fixture(3);
    for landing in 0..2 {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[("chain", p99(1.0))],
            Some(End::Complete),
        );
    }
    let empty = not_recorded(&f, |at| at.diff(&between(&f, 0, 2)));
    assert!(matches!(empty, NotRecorded::Tree(_)), "{empty:?}");
    let key = not_recorded(&f, |at| {
        at.diff(&between(&f, 0, 1).key("chian".parse().expect("key")))
    });
    assert!(matches!(key, NotRecorded::Key { .. }), "{key:?}");
    let glob = not_recorded(&f, |at| at.diff(&between(&f, 0, 1).metric("zzz*")));
    assert!(matches!(glob, NotRecorded::MetricGlob { .. }), "{glob:?}");
}

#[test]
fn equal_numbers_are_no_rows() {
    let f = fixture(2);
    for (landing, moved) in [(0, 10.0), (1, 12.0)] {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[("same", p99(5.0)), ("moved", p99(moved))],
            Some(End::Complete),
        );
    }
    let rows = ask(&f, |at| at.diff(&between(&f, 0, 1)));
    assert_eq!(
        sides(&rows),
        vec![(
            "moved".into(),
            "block.p99_ns".into(),
            Some(10.0),
            Some(12.0)
        )]
    );
    let itself = ask(&f, |at| at.diff(&between(&f, 1, 1)));
    assert_eq!(itself, Vec::new(), "a tree differs from itself in nothing");
}
