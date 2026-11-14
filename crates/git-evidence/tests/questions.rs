//! Record runs, index them, ask: each question answers from the recorded
//! runs, an open run is no value, and a name nothing records is named, never
//! read as an empty answer.

mod common;

use common::*;
use git_evidence::Declaration;
use git_evidence::NotRecorded;
use git_evidence::{End, Reason};
use git_evidence::{KeysQuery, LogQuery, ShowQuery};

fn keys_matching(glob: &str) -> KeysQuery {
    KeysQuery::default().metric(glob)
}

/// A run that learns its keys and build after opening, as a benchmark that discovers its cases does.
fn declared_run(f: &Fixture, landing: usize, flags: &str, value: f64) {
    use git_evidence::{Build, Event, EventLine, Profile};
    let run = git_evidence::RunId::new();
    let mut open = open(&f.landings[landing], &Shape::default(), &[]);
    open.keys = None;
    let events = vec![
        Event::Open(Box::new(open)),
        Declaration::new()
            .keys([key("parse_large")])
            .metric(
                git_evidence::MetricName::parse("block.p99_ns").expect("metric"),
                git_evidence::Metric::lower("ns"),
            )
            .build(Build::new().profile(Profile::new().flags(flags)))
            .into(),
        Event::result(key("parse_large"), p99(value)),
        Event::close(End::Complete),
    ];
    let lines: Vec<EventLine> = events
        .into_iter()
        .zip(0..)
        .map(|(event, seq)| event_line(&run.clone(), seq, event))
        .collect();
    f.evidence.record(&perf(), &lines).expect("record");
}

#[test]
fn a_trend_lists_each_ended_run_by_landing() {
    let f = fixture(5);
    measured_run(&f, 1, 85.0);
    measured_run(&f, 2, 88.6);
    record_run(
        &f,
        &perf(),
        4,
        &Shape::default(),
        &[("parse_large", p99(88.4))],
        None,
    );
    let points = ask(&f, |at| at.trend(&parse_large_trend(5)));
    let values: Vec<_> = points.iter().map(|p| (p.landing.get(), p.value)).collect();
    assert_eq!(
        values,
        vec![(1, 85.0), (2, 88.6)],
        "an open run is not a value"
    );
}

#[test]
fn runs_of_two_variants_are_two_rows() {
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse_large", p99(80.0))],
        Some(End::Complete),
    );
    let changed = Shape {
        harness: "h/2",
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        1,
        &changed,
        &[("parse_large", p99(40.0))],
        Some(End::Complete),
    );
    let points = ask(&f, |at| at.trend(&parse_large_trend(2)));
    let values: Vec<_> = points.iter().map(|p| (p.landing.get(), p.value)).collect();
    assert_eq!(values, vec![(0, 80.0), (1, 40.0)]);
}

#[test]
fn log_show_and_keys_answer_from_runs() {
    let f = fixture(2);
    measured_run(&f, 0, 80.0);
    let found = ask(&f, every_run);
    assert_eq!(
        (
            found[0].kind.as_str(),
            found[0].machine.as_ref().map(git_evidence::Machine::as_str),
            found[0].landing.map(git_evidence::LandingNumber::get)
        ),
        ("perf", Some("bench"), Some(0))
    );
    let run = run_ids(&f, &perf())[0].clone();
    let detail = ask(&f, |at| at.show(&ShowQuery::of(&run).key("parse_*"))).remove(0);
    assert_eq!(detail.outcomes.len(), 1, "filtered to one key");
    assert_eq!(detail.conditions, None, "a close may omit conditions");
    let unknown = not_recorded(&f, |at| {
        at.run(&"01JA8Z3K4M0000000000000000".parse().expect("run id"))
    });
    assert!(matches!(unknown, NotRecorded::Run(_)), "{unknown:?}");
    let listed = ask(&f, |at| at.keys(&keys_matching("block.*")));
    assert_eq!(listed[0].key, "parse_large");
    // Each metric carries the unit and direction its run declared, so a
    // reader can compare it without opening the run.
    let metric = &listed[0].metrics[0];
    assert_eq!(
        (metric.unit.as_str(), metric.direction),
        ("ns", Some(git_evidence::Direction::Lower)),
        "{metric:?}"
    );
}

#[test]
fn unknown_names_are_named_never_read_as_empty() {
    let f = fixture(2);
    measured_run(&f, 0, 80.0);
    let typo = not_recorded(&f, |at| {
        at.trend(&{
            let mut q = parse_large_trend(2);
            q.key = "parse_lareg".parse().expect("name");
            q
        })
    });
    let NotRecorded::Key { nearest, .. } = &typo else {
        panic!("{typo:?}")
    };
    assert!(nearest.iter().any(|n| n == "parse_large"), "{typo:?}");
    let metric = not_recorded(&f, |at| {
        at.trend(&{
            let mut q = parse_large_trend(2);
            q.metric = "p99".parse().expect("name");
            q
        })
    });
    assert!(matches!(metric, NotRecorded::Metric { .. }), "{metric:?}");
}

#[test]
fn refused_keys_give_no_rows() {
    let f = fixture(2);
    for landing in 0..2 {
        record_run(
            &f,
            &perf(),
            landing,
            &Shape::default(),
            &[("parse_large", refused("noisy_host"))],
            Some(End::Complete),
        );
    }
    // Another machine measured, so the key and metric exist.
    let elsewhere = Shape {
        machine: "other",
        ..Shape::default()
    };
    record_run(
        &f,
        &perf(),
        0,
        &elsewhere,
        &[("parse_large", p99(1.0))],
        Some(End::Complete),
    );
    let points = ask(&f, |at| {
        at.trend(&parse_large_trend(2).machine("bench".parse().expect("name")))
    });
    assert_eq!(points, Vec::new());
    let elsewhere = ask(&f, |at| at.trend(&parse_large_trend(2)));
    assert_eq!(elsewhere.len(), 1, "{elsewhere:?}");
}

#[test]
fn an_empty_store_has_nothing_recorded_for_any_key() {
    let f = fixture(1);
    let missing = not_recorded(&f, |at| at.trend(&parse_large_trend(1)));
    assert!(matches!(missing, NotRecorded::Key { .. }), "{missing:?}");
}

#[test]
fn a_build_declared_after_opening_is_part_of_the_variant() {
    // A debug build's numbers are never matched with a release build's.
    let f = fixture(2);
    declared_run(&f, 0, "release", 80.0);
    declared_run(&f, 1, "debug", 480.0);
    let points = ask(&f, |at| at.trend(&parse_large_trend(2)));
    assert_eq!(points.len(), 2, "{points:?}");
    let shown = ask(&f, |at| at.run(&points[1].run));
    assert!(
        shown
            .params
            .iter()
            .any(|p| p.name == "flags" && p.value == "debug"),
        "{shown:?}"
    );
    let [release, debug] =
        [&points[0], &points[1]].map(|p| f.evidence.at(|at| at.run(&p.run)).expect("show").variant);
    assert_ne!(release, debug);
}

#[test]
fn only_values_are_trend_rows_and_log_lists_every_run() {
    // A skip, a refusal, a run that died before declaring its keys: none is
    // a value, so none is a trend row; each is a run in the log.
    let f = fixture(2);
    measured_run(&f, 0, 80.0);
    let skipped = git_evidence::Outcome::Skipped(Reason::new("compile_error"));
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("parse_large", skipped)],
        Some(End::Complete),
    );
    record_run(
        &f,
        &perf(),
        1,
        &Shape::default(),
        &[("parse_large", refused("noisy_host"))],
        Some(End::Complete),
    );
    let mut died = run_lines(
        open(&f.landings[1], &Shape::default(), &[]),
        &[],
        Some(interrupted()),
    );
    if let git_evidence::Event::Open(open) = &mut died[0].event {
        open.keys = None;
    }
    f.evidence.record(&perf(), &died).expect("record");
    let points = ask(&f, |at| at.trend(&parse_large_trend(2)));
    let values: Vec<_> = points.iter().map(|p| (p.landing.get(), p.value)).collect();
    assert_eq!(values, vec![(0, 80.0)]);
    let logged = ask(&f, |at| at.log(&LogQuery::Tree(tree(&f, &f.landings[1]))));
    assert_eq!(logged.len(), 3, "{logged:?}");
}

#[test]
fn an_unknown_key_or_metric_in_show_and_keys_is_not_recorded() {
    let f = fixture(1);
    measured_run(&f, 0, 80.0);
    let run = run_ids(&f, &perf())[0].clone();
    let shown = not_recorded(&f, |at| at.show(&ShowQuery::of(&run).key("parse_lareg")));
    let NotRecorded::Key { nearest, .. } = &shown else {
        panic!("{shown:?}")
    };
    assert!(nearest.iter().any(|n| n == "parse_large"), "{shown:?}");
    let listed = not_recorded(&f, |at| at.keys(&keys_matching("zzz*")));
    assert!(
        matches!(listed, NotRecorded::MetricGlob { .. }),
        "{listed:?}"
    );
}

#[test]
fn a_run_whose_suite_lacks_the_key_leaves_the_value_alone() {
    // Another harness ran on the landing with no such key; the value stands.
    let f = fixture(1);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse_large", p99(80.0))],
        Some(End::Complete),
    );
    let other = Shape {
        harness: "h/other",
        ..Shape::default()
    };
    record_run(&f, &perf(), 0, &other, &[("swap", p99(1.0))], None); // still open
    record_run(&f, &perf(), 0, &other, &[], Some(End::Complete)); // keys: []
    let points = ask(&f, |at| at.trend(&parse_large_trend(1)));
    assert_eq!(points.len(), 1, "{points:?}");
}

#[test]
fn a_changed_machine_descriptor_keeps_one_variant() {
    // A kernel update describes the machine; it is not a new variant.
    let f = fixture(2);
    let mut variants = Vec::new();
    for (landing, kernel) in [(0, "6.1"), (1, "6.2")] {
        let mut opened = open(&f.landings[landing], &Shape::default(), &["parse_large"]);
        opened
            .environment
            .descriptor
            .insert("kernel".into(), serde_json::json!(kernel));
        let lines = run_lines(opened, &[("parse_large", p99(80.0))], Some(End::Complete));
        f.evidence.record(&perf(), &lines).expect("record");
        variants.push(ask(&f, |at| at.run(&lines[0].run)).variant);
    }
    assert_eq!(variants[0], variants[1]);
}

/// A run's open the index cannot parse is an error of the read, never an
/// empty label or a `null` open standing for one.
#[test]
fn an_unparseable_stored_open_refuses_the_answer() {
    let f = fixture(1);
    let lines = run_lines(
        open(&f.landings[0], &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(77.0))],
        Some(End::Complete),
    );
    f.evidence.record(&perf(), &lines).expect("record");
    let shown = lines[0].run.clone();
    assert_eq!(ask(&f, |at| at.run(&shown)).run, lines[0].run);
    rusqlite::Connection::open(cache(&f))
        .expect("index")
        .execute("UPDATE all_runs SET open = 'not json'", [])
        .expect("damage the stored open");
    let refused = f.evidence.at(|at| at.run(&shown));
    assert!(refused.is_err(), "{refused:?}");
}
