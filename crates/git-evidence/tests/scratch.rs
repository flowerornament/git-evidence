//! `testing::Scratch` isolates a test's evidence, in process and in a child.

use git_evidence::Kind;
use git_evidence::testing::Scratch;

fn kind(name: &str) -> Kind {
    Kind::parse(name).expect("kind")
}

/// `git` as a fixture runs it, touching only the repository it names
/// ([`git_process::testing::hermetic`]).
fn git() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    git_process::testing::hermetic(&mut command);
    command
}
use git_evidence::{End, Measure, Metric, MetricName, Open, Outcome};

#[test]
fn a_child_started_by_scratch_reads_the_scratch_store_and_nothing_else() {
    let scratch = Scratch::new()
        .and_then(|s| s.landings(2))
        .and_then(|s| s.machine("laptop"))
        .expect("scratch");
    let evidence = scratch.evidence();
    let metric = MetricName::parse("block.p99_ns").expect("metric");
    let mut open = Open::new(scratch.landing(1), "my-bench".parse().expect("producer"))
        .harness("my-bench/1")
        .machine(evidence.machine().expect("machine"));
    open.keys = Some(vec!["parse_large".parse().expect("key")]);
    open.suite
        .metrics
        .insert(metric.clone(), Metric::lower("ns"));
    let mut run = evidence.run(&kind("perf"), open).expect("open");
    run.result(
        &"parse_large".parse().expect("key"),
        Outcome::Measured(vec![Measure::new(metric, 5.0).expect("finite")]),
    )
    .expect("result");
    run.close(End::Complete).expect("close");
    let output = scratch
        .command(env!("CARGO_BIN_EXE_git-evidence"))
        .args([
            "--csv",
            "--trunk",
            "refs/heads/master",
            "trend",
            "block.p99_ns",
            "parse_large",
        ])
        .output()
        .expect("child");
    let text = String::from_utf8_lossy(&output.stdout);
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 2, "a header and one row: {text}");
    assert_eq!((rows[0][4], rows[1][4]), ("value", "5"), "{text}");
    assert!(Measure::new(MetricName::parse("x").expect("m"), f64::NAN).is_err());
}

#[test]
fn a_store_without_a_trunk_still_records() {
    // An open names its own commit: recording needs no landings.
    let scratch = Scratch::new()
        .and_then(|s| s.landings(1))
        .and_then(|s| s.machine("laptop"))
        .expect("scratch");
    git()
        .arg("-C")
        .arg(scratch.path())
        .args(["update-ref", "-d", "refs/heads/master"])
        .status()
        .expect("git");
    let evidence = git_evidence::Evidence::open(scratch.path().join(".git"));
    let mut open = Open::new(scratch.landing(0), "my-bench".parse().expect("producer"))
        .harness("my-bench/1")
        .machine(evidence.machine().expect("machine"));
    open.keys = Some(Vec::new());
    let line = git_evidence::EventLine::new(
        git_evidence::RunId::new(),
        0,
        git_process::Timestamp::now(),
        git_evidence::Event::Open(Box::new(open)),
    );
    let recorded = evidence
        .record(&kind("perf"), &[line])
        .expect("record without a trunk");
    assert_eq!(recorded.written, 1);
}

/// The public writer must not ask history questions, even when Git refuses them.
#[cfg(unix)]
#[test]
fn recording_and_resuming_never_query_a_trunk() {
    if let Some(dir) = std::env::var_os("NOTES_ONLY_STORE") {
        let evidence = git_evidence::Evidence::open(dir);
        let mut open = Open::new(
            git_evidence::CommitId::parse(&std::env::var("NOTES_ONLY_COMMIT").expect("commit"))
                .expect("id"),
            "writer-test".parse().expect("producer"),
        )
        .harness("writer-test/1")
        .machine("laptop".parse().expect("machine"));
        open.keys = Some(Vec::new());
        let mut line = git_evidence::EventLine::new(
            git_evidence::RunId::new(),
            0,
            git_process::Timestamp::now(),
            git_evidence::Event::Open(Box::new(open)),
        );
        let kind = kind("perf");
        evidence
            .record_dry_run(&kind, std::slice::from_ref(&line))
            .expect("dry writer");
        evidence
            .record(&kind, std::slice::from_ref(&line))
            .expect("writer");
        evidence
            .resume(&kind, &line.run)
            .expect("resume without history");
        line.seq = 1;
        line.event = git_evidence::Event::close(End::Complete);
        evidence.record(&kind, &[line]).expect("close");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new()
        .expect("scratch")
        .landings(1)
        .expect("landing");
    let bin = scratch.path().join("bin");
    std::fs::create_dir(&bin).expect("private command directory");
    let native = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("native Git");
    assert!(native.status.success());
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
for arg do
  case "$arg" in
    symbolic-ref|HEAD|refs/remotes/origin/*)
      printf '%s\n' "$arg" > "$NOTES_ONLY_TRUNK_READ"
      exit 97;;
  esac
done
exec "$NOTES_ONLY_GIT" "$@"
"#,
    )
    .expect("Git history refusal");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let marker = scratch.path().join("trunk-read");
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").expect("PATH"),
    )))
    .expect("private PATH");
    let output = scratch
        .command(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "recording_and_resuming_never_query_a_trunk",
            "--nocapture",
        ])
        .env("PATH", path)
        .env("NOTES_ONLY_STORE", scratch.path().join(".git"))
        .env("NOTES_ONLY_COMMIT", scratch.landing(0).as_str())
        .env(
            "NOTES_ONLY_GIT",
            String::from_utf8(native.stdout).expect("path").trim(),
        )
        .env("NOTES_ONLY_TRUNK_READ", &marker)
        .output()
        .expect("isolated writer");
    assert!(output.status.success(), "{output:?}");
    assert!(
        !marker.exists(),
        "no history request, including a swallowed refusal"
    );
}

/// Runs only as a child of the next test, with `GIT_DIR` pointing at a decoy.
#[test]
fn scratch_child() {
    if std::env::var_os("SCRATCH_CHILD").is_none() {
        return;
    }
    let scratch = Scratch::new()
        .and_then(|s| s.landings(1))
        .and_then(|s| s.machine("laptop"))
        .expect("scratch despite an inherited GIT_DIR");
    assert!(
        scratch.path().join(".git").is_dir(),
        "a scratch repository of its own"
    );
    let child = scratch
        .command("sh")
        .args(["-c", "printf %s \"${GIT_DIR-unset}\""])
        .output()
        .expect("sh");
    assert_eq!(
        String::from_utf8_lossy(&child.stdout),
        "unset",
        "a child cannot reach the caller"
    );
}

#[test]
fn scratch_ignores_an_inherited_git_dir_and_so_do_its_children() {
    // In a Git hook, `git rebase -x` or `git bisect run`, GIT_DIR names the
    // caller's repository; Scratch must never touch it.
    let decoy = tempfile::tempdir().expect("decoy");
    git()
        .args(["init", "-q"])
        .arg(decoy.path())
        .status()
        .expect("git init");
    let config = || std::fs::read(decoy.path().join(".git/config")).expect("config");
    let before = config();
    let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "scratch_child", "--nocapture"])
        .env("SCRATCH_CHILD", "1")
        .env("GIT_DIR", decoy.path().join(".git"))
        .status()
        .expect("child");
    assert!(status.success(), "the child's assertions held");
    assert_eq!(config(), before, "the caller's repository is untouched");
}

#[test]
fn a_misspelt_derived_metric_is_an_unknown_name() {
    // A key with a verdict knows its derived durations, and only those names.
    let scratch = Scratch::new()
        .and_then(|s| s.landings(1))
        .and_then(|s| s.machine("laptop"))
        .expect("scratch");
    let evidence = scratch.evidence();
    let mut open = Open::new(scratch.landing(0), "gates".parse().expect("producer"))
        .harness("gates/1")
        .machine(evidence.machine().expect("machine"));
    open.keys = Some(vec!["fmt".parse().expect("key")]);
    let mut run = evidence.run(&kind("gates"), open).expect("open");
    let pass = git_evidence::VerdictStatus::Pass;
    run.result(
        &"fmt".parse().expect("key"),
        Outcome::Verdict(git_evidence::Verdict::new(
            pass,
            vec![git_evidence::Try::new(pass).duration_ms(1.0)],
        )),
    )
    .expect("result");
    run.close(End::Complete).expect("close");
    let asked = |metric: &str| {
        evidence.at(|at| {
            at.trend(&git_evidence::TrendQuery::new(
                metric.parse().expect("metric"),
                "fmt".parse().expect("key"),
            ))
        })
    };
    assert_eq!(
        asked("verdict.pass.duration_ms")
            .expect("a real derived name answers")
            .len(),
        1
    );
    assert!(
        matches!(
            asked("verdict.pas.duration_ms"),
            Err(git_evidence::Error::NotRecorded(
                git_evidence::NotRecorded::Metric { .. }
            ))
        ),
        "a misspelt one is not recorded, never an empty answer"
    );
}
