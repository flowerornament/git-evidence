//! Reproducible (README "Guarantees"): the same notes and trunk give the same
//! rows, however the index was built. Catching up incrementally equals a
//! fresh rebuild, through a missing blob, a rewound or vanished ref and a run
//! recorded on two trees, whatever the fold order.

mod common;

use common::*;
use git_evidence::End;
use git_evidence::NotRecorded;

#[test]
fn incremental_catch_up_equals_a_fresh_rebuild() {
    let f = fixture(4);
    measured_run(&f, 0, 80.0);
    let first = ask(&f, |at| at.trend(&parse_large_trend(4)));
    measured_run(&f, 2, 82.0);
    measured_run(&f, 0, 81.0); // a second run on an old landing
    let incremental = ask(&f, |at| at.trend(&parse_large_trend(4)));
    assert_ne!(first, incremental);
    std::fs::remove_file(cache(&f)).expect("delete the cache");
    let fresh = ask(&f, |at| at.trend(&parse_large_trend(4)));
    assert_eq!(incremental, fresh, "the same pin, the same rows");
    let values: Vec<_> = fresh.iter().map(|p| (p.landing.get(), p.value)).collect();
    assert_eq!(
        values,
        vec![(0, 80.0), (0, 81.0), (2, 82.0)],
        "each run on a landing is a row, in the order the runs opened"
    );
}

#[test]
fn a_missing_note_blob_leaves_the_previous_index_snapshot_intact() {
    use git_evidence::TreeId;
    use std::fmt::Write as _;

    let f = fixture(3);
    measured_run(&f, 0, 80.0);
    let old_tip = ask(&f, |at| Ok(at.pin().notes["perf"].clone()));
    measured_run(&f, 1, 82.0);
    let reference = "refs/notes/evidence/perf";
    let valid_tip = git_process::Git::at(f.evidence.git_dir())
        .resolve(reference)
        .expect("resolve")
        .expect("notes");
    let listed = f
        .git
        .run(&["notes", "--ref", reference, "list"], None)
        .expect("notes");
    let mut entries = String::new();
    for line in std::str::from_utf8(&listed).expect("note entries").lines() {
        let (blob, tree) = line.split_once(' ').expect("blob and tree");
        writeln!(entries, "100644 blob {blob}\t{tree}").expect("string write");
    }
    let missing = "f".repeat(40);
    let third_tree = f.git.tree_of(f.landings[2].object()).expect("third tree");
    writeln!(entries, "100644 blob {missing}\t{third_tree}").expect("string write");
    let tree = f
        .git
        .run(&["mktree", "--missing"], Some(entries.as_bytes()))
        .expect("corrupt notes tree");
    let tree = TreeId::parse(std::str::from_utf8(&tree).expect("tree id").trim()).expect("tree");
    let broken = f
        .git
        .write_commit(&tree, &[&valid_tip], "missing note blob")
        .expect("notes commit");
    f.git
        .update_ref(reference, broken.object(), None)
        .expect("broken tip");

    let error = f
        .evidence
        .at(every_run)
        .expect_err("one missing object refuses the whole catch-up");
    assert!(error.to_string().contains(&missing), "{error}");
    let db = rusqlite::Connection::open(cache(&f)).expect("index");
    let retained_tip: String = db
        .query_row("SELECT tip FROM pin WHERE ref = 'notes:perf'", [], |row| {
            row.get(0)
        })
        .expect("old pin");
    let retained_runs: i64 = db
        .query_row("SELECT count(*) FROM all_runs", [], |row| row.get(0))
        .expect("old runs");
    assert_eq!(
        retained_tip,
        old_tip.as_str(),
        "failed catch-up cannot advance its pin"
    );
    assert_eq!(
        retained_runs, 1,
        "failed catch-up cannot partially fold valid notes"
    );
    drop(db);

    f.git
        .update_ref(reference, &valid_tip, None)
        .expect("restore notes");
    let restored = ask(&f, every_run);
    std::fs::remove_file(cache(&f)).expect("delete cache");
    assert_eq!(
        restored,
        ask(&f, every_run),
        "restored catch-up equals a fresh rebuild"
    );
}

#[test]
fn show_preserves_close_conditions_across_incremental_and_rebuilt_reads() {
    use git_evidence::Event;
    let f = fixture(1);
    let conditions = serde_json::json!({
        "tests_observed": 5128,
        "tests_retained": 20,
        "elapsed_ms": {"refused step": 1234.5},
        "certification": {"admitted": false, "reason": "missing", "kind": "perf"},
        "other": [null, true, "literal text", {"nested": []}]
    });
    for end in [End::Complete, interrupted()] {
        let mut lines = f.run_lines(0, &Shape::default(), &[], Some(end));
        let query = lines[0].run.clone();
        let close = lines.last_mut().expect("close");
        let Event::Close {
            conditions: value, ..
        } = &mut close.event
        else {
            panic!("close event")
        };
        *value = Some(conditions.clone());
        f.evidence.record(&kind("perf"), &lines[..1]).expect("open");
        let detail = ask(&f, |at| at.run(&query));
        assert_eq!(detail.conditions, None, "open has no close conditions");
        f.evidence
            .record(&kind("perf"), &lines[1..])
            .expect("close");
        for _ in 0..2 {
            let detail = ask(&f, |at| at.run(&query));
            assert_eq!(detail.conditions.as_ref(), Some(&conditions));
        }
        // An older disposable view must rebuild from the same recorded events.
        let db = rusqlite::Connection::open(
            f.git
                .git_dir()
                .join("evidence")
                .join(git_evidence::testing::index_file_name()),
        )
        .expect("index");
        db.execute("UPDATE meta SET value = '16' WHERE key = 'views'", [])
            .expect("old view version");
        drop(db);
        let detail = ask(&f, |at| at.run(&query));
        assert_eq!(detail.conditions.as_ref(), Some(&conditions));

        let output = std::process::Command::new(env!("CARGO_BIN_EXE_git-evidence"))
            .current_dir(f.dir.path())
            .env("GIT_EVIDENCE_GIT_DIR", f.git.git_dir())
            .args(["--csv", "show", query.as_str()])
            .output()
            .expect("CLI show");
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).expect("UTF-8 output");
        // One row per value: kind,machine,key,metric,value,unit,run.
        let value = |metric: &str| {
            text.lines()
                .map(|line| line.split(',').collect::<Vec<_>>())
                .find(|cells| cells[3] == metric)
                .map(|cells| cells[4].to_owned())
        };
        assert_eq!(value("condition.tests_observed").as_deref(), Some("5128"));
        assert_eq!(
            value("condition.certification.reason").as_deref(),
            Some("missing")
        );
        assert_eq!(
            value("condition.other.0").as_deref(),
            Some(""),
            "null is empty"
        );
        assert_eq!(value("condition.other.3.nested").as_deref(), Some("[]"));

        let db = rusqlite::Connection::open(
            f.git
                .git_dir()
                .join("evidence")
                .join(git_evidence::testing::index_file_name()),
        )
        .expect("index");
        db.execute(
            "UPDATE all_runs SET conditions = '{broken' WHERE run = ?1",
            [&query],
        )
        .expect("corrupt only the disposable view");
        let error = f
            .evidence
            .at(|at| at.run(&query))
            .expect_err("invalid JSON must refuse");
        assert!(
            matches!(
                error,
                git_evidence::Error::Index(git_evidence::IndexError::Sql(
                    rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, _)
                ))
            ),
            "{error:?}"
        );
        db.execute("UPDATE meta SET value = '16' WHERE key = 'views'", [])
            .expect("rebuild restores the recorded value");
        drop(db);
        let detail = ask(&f, |at| at.run(&query));
        assert_eq!(detail.conditions.as_ref(), Some(&conditions));
    }
}

#[test]
fn a_run_recorded_on_two_trees_is_excluded_from_both_whatever_the_fold_order() {
    // The same pin gives the same rows however the index was built.
    let f = fixture(2);
    let shared = git_evidence::RunId::derived(7, b"one id");
    for (landing, value) in [(1, 81.0), (0, 80.0)] {
        let mut lines = run_lines(
            open(&f.landings[landing], &Shape::default(), &["parse_large"]),
            &[("parse_large", p99(value))],
            Some(End::Complete),
        );
        for line in &mut lines {
            line.run = shared.clone();
        }
        // Each copy closes with conditions of its own, so a row the
        // exclusion failed to remove would show which copy left it.
        if let Some(git_evidence::Event::Close { conditions, .. }) =
            lines.last_mut().map(|l| &mut l.event)
        {
            *conditions = Some(serde_json::json!({"copy": landing}));
        }
        // One run id on two trees is what a merge from another store can
        // bring; `record` refuses it here, so it arrives raw.
        append_raw(&f, &perf(), landing, &lines);
        // Read between the two, so the index folds the trees one at a time.
        ask(&f, every_run);
    }
    // The excluded run leaves no close-condition row behind, in the index's
    // own table as in the view (which reads only counted runs).
    let condition_rows = || {
        f.evidence
            .sql(&format!(
                "SELECT count(*) FROM run_conditions WHERE run = '{}'",
                shared.as_str()
            ))
            .expect("sql")
            .rows[0][0]
            .cell()
    };
    assert_eq!(condition_rows().as_deref(), Some("0"), "incrementally");
    let trend = || {
        f.evidence
            .at(|at| at.trend(&parse_large_trend(2)))
            .map_err(|e| e.to_string())
    };
    let incremental = (ask(&f, every_run), trend());
    std::fs::remove_file(cache(&f)).expect("delete the cache");
    let fresh = (ask(&f, every_run), trend());
    assert_eq!(incremental, fresh, "the same pin, the same rows");
    assert_eq!(condition_rows().as_deref(), Some("0"), "fresh");
    let (_, trend) = fresh;
    assert!(
        trend
            .as_ref()
            .is_err_and(|e| e.contains("no run records a key matching")),
        "neither copy is a value: {trend:?}"
    );
    let shown = not_recorded(&f, |at| at.run(&shared));
    assert!(matches!(shown, NotRecorded::Unreadable { .. }), "{shown:?}");
}

#[test]
fn record_refuses_a_run_already_recorded_on_another_tree() {
    // Where `record` can see the first copy, it refuses the second outright.
    let f = fixture(2);
    let shared = git_evidence::RunId::derived(7, b"one id");
    let on = |landing: usize| {
        let mut lines = run_lines(
            open(&f.landings[landing], &Shape::default(), &["parse_large"]),
            &[("parse_large", p99(80.0))],
            Some(End::Complete),
        );
        for line in &mut lines {
            line.run = shared.clone();
        }
        lines
    };
    f.evidence.record(&perf(), &on(0)).expect("first tree");
    let refused = f
        .evidence
        .record(&perf(), &on(1))
        .expect_err("a second tree")
        .to_string();
    assert!(refused.contains("is already recorded on tree"), "{refused}");
    let runs = ask(&f, every_run);
    assert_eq!(
        runs.iter().filter(|r| r.run == shared).count(),
        1,
        "only the first copy is held: {runs:?}"
    );
}

#[test]
fn a_copy_that_stops_folding_still_excludes_the_others() {
    // A copy of a run on another tree, then broken there, gives the same
    // rows incrementally and from a rebuild.
    let f = fixture(2);
    let lines = run_lines(
        open(&f.landings[1], &Shape::default(), &["parse_large"]),
        &[("parse_large", p99(80.0))],
        Some(End::Complete),
    );
    f.evidence.record(&perf(), &lines).expect("record");
    ask(&f, every_run);
    let append = |text: String| {
        let other = tree(&f, &f.landings[0]);
        let status = git_process::testing::hermetic(&mut std::process::Command::new("git"))
            .arg("--git-dir")
            .arg(f.git.git_dir())
            .args([
                "notes",
                "--ref",
                "refs/notes/evidence/perf",
                "append",
                "--no-separator",
                "-m",
            ])
            .arg(text)
            .arg(other.as_str())
            .status()
            .expect("git");
        assert!(status.success());
    };
    // Another store's merge brought a copy of the run onto landing 0's tree…
    let copy: Vec<String> = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect();
    append(copy.join("\n"));
    ask(&f, every_run);
    // …and then a different event at seq 1 there, so that copy no longer folds.
    let mut broken = lines[1].clone();
    broken.time = "2000-01-01T00:00:00.000Z".into();
    append(serde_json::to_string(&broken).expect("json"));
    let incremental = ask(&f, every_run);
    std::fs::remove_file(cache(&f)).expect("delete the cache");
    let fresh = ask(&f, every_run);
    assert_eq!(incremental, fresh, "the same pin, the same rows");
}

#[test]
fn a_rewound_or_vanished_ref_reads_as_a_rebuild() {
    // Outside grow-only use: an honest mistake must not leave stale state.
    let f = fixture(2);
    record_run(
        &f,
        &perf(),
        0,
        &Shape::default(),
        &[("parse_large", p99(80.0))],
        Some(End::Complete),
    );
    let shared = git_evidence::RunId::derived(9, b"copy");
    let copy_on = |landing: usize| {
        let mut lines = run_lines(
            open(&f.landings[landing], &Shape::default(), &["parse_large"]),
            &[("parse_large", p99(81.0))],
            Some(End::Complete),
        );
        for line in &mut lines {
            line.run = shared.clone();
        }
        // A copy on another tree arrives as a merge would bring it: raw.
        append_raw(&f, &perf(), landing, &lines);
    };
    copy_on(0);
    let before = git_process::Git::at(f.evidence.git_dir())
        .resolve("refs/notes/evidence/perf")
        .expect("resolve")
        .expect("ref");
    copy_on(1); // now on two trees: excluded
    ask(&f, every_run);
    let git = |args: &[&str]| {
        assert!(
            git_process::testing::hermetic(&mut std::process::Command::new("git"))
                .arg("--git-dir")
                .arg(f.git.git_dir())
                .args(args)
                .status()
                .expect("git")
                .success()
        );
    };
    git(&["update-ref", "refs/notes/evidence/perf", before.as_str()]);
    let rewound = ask(&f, every_run);
    assert_eq!(rewound.len(), 2, "the single copy is readable again");
    std::fs::remove_file(cache(&f)).expect("delete the cache");
    assert_eq!(rewound, ask(&f, every_run), "the same rows after a rewind");
    git(&["update-ref", "-d", "refs/notes/evidence/perf"]);
    let gone = ask(&f, every_run);
    std::fs::remove_file(cache(&f)).expect("delete the cache");
    assert_eq!(
        gone,
        ask(&f, every_run),
        "the same rows after the ref vanished"
    );
}
