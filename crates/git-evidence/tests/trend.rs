//! `trend METRIC KEY`: one row per run's value, each naming its run's kind
//! and variant, so values `diff` would never compare (another harness, other
//! parameters) never read as one series; `--variant` keeps one series.

mod common;

use common::*;
use git_evidence::{End, TrendQuery};

fn chain() -> TrendQuery {
    TrendQuery::new("block.p99_ns".parse().expect("metric"), key("chain")).landings(newest(2))
}

/// `trend block.p99_ns chain --csv` with `args` added, as rows of cells.
fn csv(f: &Fixture, args: &[&str]) -> Vec<Vec<String>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["--csv", "--trunk", "refs/heads/master", "trend"])
        .args(["block.p99_ns", "chain"])
        .args(args)
        .output()
        .expect("CLI");
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| line.split(',').map(str::to_owned).collect())
        .collect()
}

#[test]
fn runs_of_two_variants_name_their_variants_and_one_can_be_kept() {
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

    let points = ask(&f, |at| at.trend(&chain()));
    assert_eq!(points.len(), 2, "{points:?}");
    assert_ne!(points[0].variant, points[1].variant, "{points:?}");
    assert!(points.iter().all(|p| p.kind == perf()));

    let rows = csv(&f, &[]);
    assert_eq!(
        rows[0],
        [
            "landing", "commit", "date", "kind", "machine", "variant", "value", "unit", "run"
        ]
    );
    let variants: Vec<&str> = rows[1..].iter().map(|row| row[5].as_str()).collect();
    assert_eq!(
        variants,
        [points[0].short_variant(), points[1].short_variant()]
    );
    assert!(variants.iter().all(|v| v.len() == 8), "{rows:?}");
    assert!(rows[1..].iter().all(|row| row[3] == "perf"), "{rows:?}");

    // One series: the variant the second row printed, through the API and the CLI.
    let kept = points[1].short_variant().to_owned();
    let one = ask(&f, |at| at.trend(&chain().variant(kept.clone())));
    assert_eq!(one.len(), 1, "{one:?}");
    assert!((one[0].value - 50.0).abs() < f64::EPSILON);
    let rows = csv(&f, &["--variant", &kept]);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1][6], "50", "{rows:?}");

    // The prefix in capitals is the same variant; one that is not hex is refused.
    assert_eq!(csv(&f, &["--variant", &kept.to_uppercase()]).len(), 2);
    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .current_dir(f.dir.path())
        .args(["trend", "block.p99_ns", "chain", "--variant", "not-hex"])
        .output()
        .expect("CLI");
    assert_ne!(refused.status.code(), Some(0), "{refused:?}");

    // A kind that recorded nothing for the key gives no rows.
    assert!(ask(&f, |at| at.trend(&chain().kind("checks"))).is_empty());
}
