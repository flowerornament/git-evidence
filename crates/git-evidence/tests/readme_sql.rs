//! Every SQL example in the README runs against a fixture store, and its rows
//! are asserted. An
//! example added to the README without an expectation here fails, and so does
//! an expectation whose example is gone.

mod common;

use common::*;
use git_evidence::{End, Measure, MetricName, Outcome, Reason, Try, Verdict, VerdictStatus};

/// Four landings, 2026-10-01 to 10-04. On laptop: perf measured both metrics
/// on landings 0 and 1, and on landing 2 refused one key and skipped the other
/// (a complete run without the metric). Checks of the full suite on landings
/// 1 and 2, and of the quick suite on 2.
fn store() -> Fixture {
    let f = fixture(4);
    let laptop = Shape {
        producer: "bench-suite",
        machine: "laptop",
        ..Shape::default()
    };
    let both = |ours: f64, reference: f64| {
        let measure = |metric: &str, value| {
            Measure::new(MetricName::parse(metric).expect("metric"), value).expect("finite")
        };
        Outcome::Measured(vec![
            measure("latency.p99_ns", ours),
            measure("reference.latency.p99_ns", reference),
        ])
    };
    for (landing, large, small) in [(0, 88_000.0, 12_000.0), (1, 90_200.0, 12_100.0)] {
        record_run(
            &f,
            &perf(),
            landing,
            &laptop,
            &[
                ("parse_large", both(large, 41_000.0)),
                ("parse_small", both(small, 10_000.0)),
            ],
            Some(End::Complete),
        );
    }
    record_run(
        &f,
        &perf(),
        2,
        &laptop,
        &[
            ("parse_large", Outcome::Skipped(Reason::new("build_failed"))),
            ("parse_small", refused("noisy_host")),
        ],
        Some(End::Complete),
    );
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
    let suite = |name| Shape {
        producer: "checker",
        machine: "laptop",
        params: vec![("suite", name)],
        ..Shape::default()
    };
    for landing in [1, 2] {
        record_run(
            &f,
            &kind("checks"),
            landing,
            &suite("full"),
            &[
                (
                    "lint",
                    verdict(VerdictStatus::Pass, Some(812.0), Some(2310.0)),
                ),
                ("unit", verdict(VerdictStatus::Fail, Some(4000.0), None)),
            ],
            Some(End::Complete),
        );
    }
    record_run(
        &f,
        &kind("checks"),
        2,
        &suite("quick"),
        &[("lint", verdict(VerdictStatus::Pass, Some(5.0), Some(5.0)))],
        Some(End::Complete),
    );
    f
}

/// Each ```sql block of the README, by its first line (a `--` comment).
fn examples() -> Vec<(String, String)> {
    let readme = include_str!("../README.md");
    let mut found = Vec::new();
    let mut lines = readme.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "```sql" {
            continue;
        }
        let body: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != "```").collect();
        let title = body
            .first()
            .and_then(|l| l.strip_prefix("-- "))
            .expect("each example starts with a -- comment naming it");
        found.push((title.to_owned(), body.join("\n")));
    }
    found
}

/// What each example returns on [`store`]: its header, then its rows.
fn expected(title: &str) -> Option<&'static [&'static str]> {
    Some(match title {
        "ours against the reference per key, in the newest complete run on laptop that measured ours" => {
            &[
                "key,ours,reference,ratio,landing,conditions",
                "parse_large,90200,41000,2.2,1,quiet",
                "parse_small,12100,10000,1.21,1,quiet",
            ]
        }
        "verdict time per key of the full suite over the last 30 landings" => &[
            "landing,key,status,duration_ms,cpu_ms",
            "1,lint,pass,812,2310",
            "1,unit,fail,4000,",
            "2,lint,pass,812,2310",
            "2,unit,fail,4000,",
        ],
        "keys refused or skipped on the landings of October 2026" => &[
            "landing,kind,key,outcome,reason",
            "2,perf,parse_large,skipped,build_failed",
            "2,perf,parse_small,refused,noisy_host",
        ],
        _ => return None,
    })
}

#[test]
fn every_readme_sql_example_returns_its_documented_rows() {
    let f = store();
    let examples = examples();
    assert_eq!(
        examples.len(),
        3,
        "an example was added or removed: update `expected`"
    );
    for (title, query) in &examples {
        let want = expected(title).unwrap_or_else(|| panic!("no expectation for {title:?}"));
        let rows = f
            .evidence
            .sql(query)
            .unwrap_or_else(|e| panic!("{title}: {e}"));
        let mut got = vec![rows.columns.join(",")];
        got.extend(rows.rows.iter().map(|row| {
            row.iter()
                .map(|v| v.cell().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(",")
        }));
        assert_eq!(got, want, "{title}");
    }
}
