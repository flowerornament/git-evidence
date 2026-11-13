//! `git evidence` with no verb: the newest landings, oldest first, each with
//! its run count of each kind, refused runs counted apart.
mod common;

use common::*;
use git_evidence::{End, Outcome, Try, Verdict, VerdictStatus};
use git_evidence::{RunCount, TidyRows};

fn verdict(status: VerdictStatus) -> Outcome {
    Outcome::Verdict(Verdict {
        status,
        tries: vec![Try {
            status,
            duration_ms: None,
            cpu_ms: None,
        }],
    })
}

#[test]
fn the_overview_counts_each_landings_runs_newest_first() {
    let f = fixture(3);
    let gates = Shape {
        producer: "ci-checks",
        ..Shape::default()
    };
    let mut lines = f.run_lines(
        1,
        &gates,
        &[("test", verdict(VerdictStatus::Pass))],
        Some(End::Complete),
    );
    lines.extend(f.run_lines(
        2,
        &gates,
        &[("test", verdict(VerdictStatus::Fail))],
        Some(End::Complete),
    ));
    f.evidence.record(&kind("gates"), &lines).expect("setup");
    let perf_lines = f.run_lines(
        0,
        &Shape::default(),
        &[("chain", measured("runtime.block.mean_ns", 100.0))],
        Some(End::Complete),
    );
    f.evidence.record(&perf(), &perf_lines).expect("setup");

    f.evidence
        .record(
            &perf(),
            &f.run_lines(
                0,
                &Shape::default(),
                &[("chain", refused("noisy_host"))],
                Some(End::Refused(git_evidence::Reason::new("noisy_host", None))),
            ),
        )
        .expect("setup");

    let overview = ask(&f, |at| {
        at.overview(std::num::NonZeroU32::new(8).expect("8"))
    });
    let positions: Vec<u32> = overview
        .landings
        .iter()
        .map(|l| l.landing.number.get())
        .collect();
    assert_eq!(positions, vec![0, 1, 2], "oldest first, as log and trend");
    let one = RunCount {
        runs: 1,
        refused: 0,
    };
    assert_eq!(overview.landings[2].runs["gates"], one);
    assert_eq!(overview.landings[1].runs["gates"], one);
    assert_eq!(
        overview.landings[0].runs["perf"],
        RunCount {
            runs: 2,
            refused: 1
        }
    );
    assert_eq!(
        overview.header().join(","),
        "landing,commit,date,gates,perf,perf_refused,subject"
    );
    let rows = overview.rows();
    assert_eq!(
        rows[2][3..6],
        [
            Some("1".to_owned()),
            Some("0".to_owned()),
            Some("0".to_owned())
        ],
        "a kind with no run on a landing counts 0"
    );
    assert_eq!(rows[0][4..6], [Some("2".to_owned()), Some("1".to_owned())]);
}
