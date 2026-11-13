//! Declared metrics: a number's unit and direction come from its run's
//! declaration, units are never converted, and verdict durations are derived
//! when evidence is read, never stored; a missing one is absent, never zero.

mod common;

use common::*;
use git_evidence::Declaration;
use git_evidence::Problem;
use git_evidence::TrendQuery;
use git_evidence::{End, Event, EventLine, Metric, Outcome, Try, Verdict, VerdictStatus};
use git_evidence::{MetricName, RunId};
use git_evidence::{RecordError, Refusal};

fn metric(name: &str) -> MetricName {
    MetricName::parse(name).expect("metric")
}

fn ns() -> Metric {
    Metric::lower("ns")
}

fn refusal(lines: &[EventLine], f: &Fixture) -> Refusal {
    match f.evidence.record(&perf(), lines) {
        Err(git_evidence::Error::Record(RecordError::Refused(why))) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn record_refuses_what_its_declarations_do_not_cover_and_writes_nothing() {
    let f = fixture(1);
    // A measure of a metric the run never declared.
    let lines = vec![event_line(
        &RunId::new(),
        0,
        Event::Open(Box::new(open(
            &f.landings[0],
            &Shape::default(),
            &["parse_large"],
        ))),
    )];
    let run = lines[0].run.clone();
    let mut undeclared = lines.clone();
    undeclared.push(event_line(
        &run.clone(),
        1,
        Event::result(key("parse_large"), p99(1.0)),
    ));
    let refused = refusal(&undeclared, &f);
    assert!(
        matches!(
            refused,
            Refusal::Rule {
                problem: Problem::UndeclaredMetric { .. },
                ..
            }
        ),
        "{refused}"
    );
    assert!(
        refused.to_string().ends_with(refused.remedy()),
        "every refusal names its remedy"
    );
    // A redefinition and a derived name, declared late.
    for (name, declared, expect) in [
        ("block.p99_ns", Metric::new("us", ns().better), "redefined"),
        ("verdict.pass.duration_ms", ns(), "derived metric"),
    ] {
        let mut opened = open(&f.landings[0], &Shape::default(), &["parse_large"]);
        opened.suite.metrics.insert(metric("block.p99_ns"), ns());
        let run = RunId::new();
        let lines = vec![
            event_line(&run.clone(), 0, Event::Open(Box::new(opened))),
            event_line(
                &run,
                1,
                Declaration::default().metric(metric(name), declared).into(),
            ),
        ];
        let refused = refusal(&lines, &f);
        assert!(
            matches!(
                (&refused, expect),
                (
                    Refusal::Rule {
                        problem: Problem::MetricRedefined { .. },
                        ..
                    },
                    "redefined"
                ) | (
                    Refusal::Rule {
                        problem: Problem::ReservedMetric { .. },
                        ..
                    },
                    "derived metric"
                )
            ),
            "{name}: {refused}"
        );
    }
    // The previous envelope version.
    let mut old = run_lines(
        open(&f.landings[0], &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(1.0))],
        Some(End::Complete),
    );
    for line in &mut old {
        line.v = 1;
    }
    assert!(matches!(
        refusal(&old, &f),
        Refusal::Envelope { found: 1, .. }
    ));
    assert_eq!(note(&f, &perf(), &tree(&f, &f.landings[0])), None);
}

#[test]
fn a_metric_recorded_in_two_units_keeps_each_unit() {
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse_large", p99(80_000.0))],
        Some(End::Complete),
    );
    let micros = Shape {
        unit: "us",
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        1,
        &micros,
        &[("parse_large", p99(81.0))],
        Some(End::Complete),
    );
    let answer = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "block.p99_ns".parse().expect("name"),
                "parse_large".parse().expect("name"),
            )
            .landings(newest(10)),
        )
    });
    let units: Vec<(f64, &str)> = answer.iter().map(|p| (p.value, p.unit.as_str())).collect();
    assert_eq!(units, vec![(80_000.0, "ns"), (81.0, "us")], "{answer:?}");
}

fn verdict(status: VerdictStatus, durations: &[Option<f64>]) -> Outcome {
    let tries = durations
        .iter()
        .map(|d| {
            let mut t = Try::new(status);
            t.duration_ms = *d;
            t
        })
        .collect();
    Outcome::Verdict(Verdict::new(status, tries))
}

#[test]
fn verdict_durations_are_derived_per_status_and_never_guessed() {
    let f = fixture(1);
    record_run(
        &f,
        &kind("gates"),
        0,
        &Shape::default(),
        &[
            ("fmt", verdict(VerdictStatus::Pass, &[Some(1.5), Some(2.5)])),
            ("clippy", verdict(VerdictStatus::Fail, &[Some(10.0)])),
            ("nextest", verdict(VerdictStatus::Pass, &[Some(1.0), None])),
            ("doc", verdict(VerdictStatus::Pass, &[])),
            (
                "huge",
                verdict(VerdictStatus::Timeout, &[Some(f64::MAX), Some(f64::MAX)]),
            ),
        ],
        Some(End::Complete),
    );
    let run = run_ids(&f, &kind("gates"))[0].clone();
    let detail = ask(&f, |at| at.run(&run));
    let measures = |key: &str| {
        detail
            .outcomes
            .iter()
            .find(|o| o.key.as_str() == key)
            .map(|o| serde_json::to_value(o.outcome.measures()).expect("measures"))
            .expect("outcome")
    };
    assert_eq!(
        measures("fmt"),
        serde_json::json!([{"metric": "verdict.pass.duration_ms", "value": 4.0, "unit": "ms",
            "better": "lower", "n": 2, "mad": null, "origin": "derived"}])
    );
    assert_eq!(
        measures("clippy")[0]["metric"],
        "verdict.fail.duration_ms",
        "a fail is its own metric"
    );
    for key in ["nextest", "doc", "huge"] {
        assert_eq!(
            measures(key),
            serde_json::json!([]),
            "{key}: missing or overflowing is absent, never zero"
        );
    }
}

fn timed(status: VerdictStatus, tries: &[(f64, Option<f64>)]) -> Outcome {
    let tries = tries
        .iter()
        .map(|(wall, cpu)| {
            let mut t = Try::new(status).duration_ms(*wall);
            t.cpu_ms = *cpu;
            t
        })
        .collect();
    Outcome::Verdict(Verdict::new(status, tries))
}

#[test]
fn verdict_cpu_times_are_summed_like_durations_and_absent_is_never_zero() {
    let f = fixture(1);
    record_run(
        &f,
        &kind("gates"),
        0,
        &Shape::default(),
        &[
            // Parallel work: more CPU than wall time, recorded as observed.
            (
                "build",
                timed(
                    VerdictStatus::Pass,
                    &[(10.0, Some(30.0)), (5.0, Some(15.5))],
                ),
            ),
            ("fmt", timed(VerdictStatus::Pass, &[(2.0, None)])),
            (
                "doc",
                timed(VerdictStatus::Fail, &[(1.0, Some(3.0)), (1.0, None)]),
            ),
        ],
        Some(End::Complete),
    );
    let run = run_ids(&f, &kind("gates"))[0].clone();
    let detail = ask(&f, |at| at.run(&run));
    let measures = |key: &str| {
        detail
            .outcomes
            .iter()
            .find(|o| o.key.as_str() == key)
            .map(|o| serde_json::to_value(o.outcome.measures()).expect("measures"))
            .expect("outcome")
    };
    let cpu = |key: &str| {
        measures(key)
            .as_array()
            .expect("measures")
            .iter()
            .find(|m| m["metric"].as_str().is_some_and(|m| m.ends_with(".cpu_ms")))
            .cloned()
    };
    assert_eq!(
        cpu("build"),
        Some(
            serde_json::json!({"metric": "verdict.pass.cpu_ms", "value": 45.5, "unit": "ms",
            "better": "lower", "n": 2, "mad": null, "origin": "derived"})
        )
    );
    for key in ["fmt", "doc"] {
        assert_eq!(
            cpu(key),
            None,
            "{key}: a try without CPU time is absent, never zero"
        );
        assert!(
            measures(key)
                .as_array()
                .expect("measures")
                .iter()
                .any(|m| m["metric"]
                    .as_str()
                    .is_some_and(|m| m.ends_with(".duration_ms"))),
            "{key}: its wall time still stands"
        );
    }
    // Asked for by name, the missing CPU time is a known derived metric with
    // no row, not an unknown metric.
    let points = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "verdict.pass.cpu_ms".parse().expect("name"),
                "fmt".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert_eq!(points, Vec::new());
}

#[test]
fn a_recorded_measure_reads_back_with_its_declaration_and_origin() {
    let f = fixture(1);
    let shape = Shape {
        unit: "{count}",
        better: None,
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        0,
        &shape,
        &[(
            "parse_large",
            Outcome::measured(metric("plan.regions"), 3.0).expect("finite"),
        )],
        Some(End::Complete),
    );
    let answer = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "plan.regions".parse().expect("name"),
                "parse_large".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert_eq!(answer[0].unit, "{count}");
    let run = answer[0].run.clone();
    let shown = ask(&f, |at| at.run(&run));
    assert_eq!(shown.outcomes[0].outcome.measures()[0].better, None);
}

#[test]
fn a_missing_derived_duration_is_no_row_and_no_error() {
    // A pass whose try has no duration has no duration: no row, not zero.
    let f = fixture(1);
    record_run(
        &f,
        &kind("gates"),
        0,
        &Shape::default(),
        &[("fmt", verdict(VerdictStatus::Pass, &[None]))],
        Some(End::Complete),
    );
    let answer = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "verdict.pass.duration_ms".parse().expect("name"),
                "fmt".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert_eq!(answer, Vec::new());
}

#[test]
fn a_line_of_another_envelope_version_is_a_violation_at_read() {
    // A v1 line that reached a note (an old store, a fetch) is never read as v2.
    let f = fixture(1);
    let mut lines = run_lines(
        open(&f.landings[0], &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(1.0))],
        Some(End::Complete),
    );
    for line in &mut lines {
        line.v = 1;
    }
    let raw: Vec<EventLine> = lines;
    append_raw(&f, &perf(), 0, &raw);
    let answer = f.evidence.at(|at| {
        at.trend(
            &TrendQuery::new(
                "block.p99_ns".parse().expect("name"),
                "parse_large".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert!(answer.is_err(), "the run is not read: {answer:?}");
    let audit = f.evidence.audit().expect("audit");
    assert!(!audit.findings.is_empty(), "{audit:?}");
}

#[test]
fn a_verdict_of_another_status_has_no_pass_duration() {
    // A failed test has no pass duration: no row; its fail duration is a row.
    let f = fixture(1);
    record_run(
        &f,
        &kind("gates"),
        0,
        &Shape::default(),
        &[("fmt", verdict(VerdictStatus::Fail, &[Some(1.0)]))],
        Some(End::Complete),
    );
    let answer = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "verdict.pass.duration_ms".parse().expect("name"),
                "fmt".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert_eq!(answer, Vec::new());
    let failed = ask(&f, |at| {
        at.trend(
            &TrendQuery::new(
                "verdict.fail.duration_ms".parse().expect("name"),
                "fmt".parse().expect("name"),
            )
            .landings(newest(1)),
        )
    });
    assert_eq!(failed.len(), 1);
}
