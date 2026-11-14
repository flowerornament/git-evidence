//! Fixtures shared by the integration tests: a trunk of landings with distinct
//! trees; whole runs recorded through the public `record` door; questions
//! asked at a pin.
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the fixtures"
)]

use git_evidence::At;
use git_evidence::Recorded;
use git_evidence::{CommitId, Key, Kind, MetricName, ProducerName, RunId, TreeId};
use git_evidence::{Direction, End, Event, EventLine, Measure, Metric, Open, Outcome, Reason};
use git_evidence::{Error, Evidence};
use git_process::TreeEntry;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Plain Git over a fixture's repository, for building trunks and planting
/// bytes the crate would refuse: the crate's own [`git_process::Git`]
/// only reads outside it, so tests bring their own writer.
#[derive(Debug, Clone)]
pub struct RawGit {
    git_dir: std::path::PathBuf,
}

impl RawGit {
    pub fn at(git_dir: impl Into<std::path::PathBuf>) -> Self {
        // A caller-installed observer takes precedence over fixture diagnostics.
        let _ = git_process::observe_start_failures::<git_process::StartFailureLog>();
        Self {
            git_dir: git_dir.into(),
        }
    }

    pub fn git_dir(&self) -> &std::path::Path {
        &self.git_dir
    }

    pub fn output(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut command = Command::new("git");
        git_process::testing::hermetic(&mut command);
        // Started as the crate starts git, so a loaded machine's refused
        // start is tried again, not a failed test.
        git_process::output_of(
            command
                .arg("--git-dir")
                .arg(&self.git_dir)
                .args(args)
                .stdin(std::process::Stdio::null()),
        )
        .map_err(std::io::Error::other)
    }

    /// `git args` with `stdin`; its standard output, or the failure as text.
    pub fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        self.run_with(args, stdin, &[])
    }

    /// As [`RawGit::run`], with `env` set for Git.
    pub fn run_with(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        env: &[(&str, String)],
    ) -> Result<Vec<u8>, String> {
        use std::io::Write as _;
        let mut command = Command::new("git");
        git_process::testing::hermetic(&mut command);
        command.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
        let mut child = git_process::start(
            command
                .arg("--git-dir")
                .arg(&self.git_dir)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()),
        )
        .map_err(|e| e.to_string())?;
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(stdin.unwrap_or_default())
                .map_err(|e| e.to_string())?;
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn text(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        self.run(args, stdin)
            .map(|out| String::from_utf8_lossy(&out).trim().to_owned())
    }

    pub fn write_blob(&self, bytes: &[u8]) -> Result<git_evidence::ObjectId, String> {
        let id = self.text(&["hash-object", "-w", "--stdin"], Some(bytes))?;
        git_evidence::ObjectId::parse(&id).map_err(|e| e.to_string())
    }

    pub fn write_tree(&self, entries: &[TreeEntry]) -> Result<TreeId, String> {
        let mut input = Vec::new();
        for entry in entries {
            input.extend_from_slice(
                format!("{} {} {}\t{}", entry.mode, entry.kind, entry.id, entry.name).as_bytes(),
            );
            input.push(0);
        }
        TreeId::parse(&self.text(&["mktree", "-z"], Some(&input))?).map_err(|e| e.to_string())
    }

    pub fn write_commit(
        &self,
        tree: &TreeId,
        parents: &[&git_evidence::ObjectId],
        message: &str,
    ) -> Result<CommitId, String> {
        self.write_commit_at(tree, parents, message, None)
    }

    /// A commit dated `seconds` (Unix time, UTC) when given, else now.
    pub fn write_commit_at(
        &self,
        tree: &TreeId,
        parents: &[&git_evidence::ObjectId],
        message: &str,
        seconds: Option<i64>,
    ) -> Result<CommitId, String> {
        let mut args = vec!["commit-tree", tree.as_str()];
        for parent in parents {
            args.extend(["-p", parent.as_str()]);
        }
        args.extend(["-m", message]);
        let env: Vec<(&str, String)> = seconds
            .map(|s| {
                ["GIT_COMMITTER_DATE", "GIT_AUTHOR_DATE"]
                    .map(|name| (name, format!("@{s} +0000")))
                    .to_vec()
            })
            .unwrap_or_default();
        let out = self.run_with(&args, None, &env)?;
        CommitId::parse(String::from_utf8_lossy(&out).trim()).map_err(|e| e.to_string())
    }

    pub fn update_ref(
        &self,
        reference: &str,
        new: &git_evidence::ObjectId,
        _expected: Option<&git_evidence::ObjectId>,
    ) -> Result<(), String> {
        self.run(&["update-ref", reference, new.as_str()], None)
            .map(|_| ())
    }

    pub fn tree_of(&self, commit: &git_evidence::ObjectId) -> Result<TreeId, String> {
        TreeId::parse(&self.text(&["rev-parse", &format!("{commit}^{{tree}}")], None)?)
            .map_err(|e| e.to_string())
    }
}

/// The committer time of a fixture's landing 0: 2026-10-01T12:00:00Z. Landing
/// `n` is dated `n` days later.
pub const FIRST_LANDING: i64 = 1_790_856_000;
pub const DAY: i64 = 86_400;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub git: RawGit,
    pub evidence: Evidence,
    pub landings: Vec<CommitId>,
}

impl Fixture {
    /// Complete event lines for one distinct run on this fixture's landing.
    /// Several setup runs may be recorded together through the public writer;
    /// intermediate observations still require their own recording boundary.
    pub fn run_lines(
        &self,
        landing: usize,
        shape: &Shape,
        results: &[(&str, Outcome)],
        end: Option<End>,
    ) -> Vec<EventLine> {
        let keys: Vec<&str> = results.iter().map(|(key, _)| *key).collect();
        let opened = declaring(open(&self.landings[landing], shape, &keys), shape, results);
        run_lines(opened, results, end)
    }
}

/// A trunk (`refs/heads/master`) of `landings` landings, each with its own
/// tree.
pub fn fixture(landings: usize) -> Fixture {
    // A caller-installed observer takes precedence over fixture diagnostics.
    let _ = git_process::observe_start_failures::<git_process::StartFailureLog>();
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        git_process::start(
            git_process::testing::hermetic(&mut Command::new("git"))
                .args(["init", "-q", "--initial-branch=master"])
                .arg(dir.path())
        )
        .expect("start init")
        .wait()
        .expect("init")
        .success()
    );
    git_process::testing::fixture_config(&git_process::Git::at(dir.path().join(".git")))
        .expect("fixture config");
    let git = RawGit::at(dir.path().join(".git"));
    let mut commits = Vec::new();
    let mut parent: Option<CommitId> = None;
    for n in 0..landings {
        let marker = git
            .write_blob(format!("landing {n}\n").as_bytes())
            .expect("blob");
        let entries = vec![TreeEntry {
            mode: "100644".into(),
            kind: "blob".into(),
            id: marker,
            name: "landing".into(),
        }];
        let tree = git.write_tree(&entries).expect("tree");
        let parents: Vec<_> = parent.iter().map(CommitId::object).collect();
        let seconds = FIRST_LANDING + DAY * i64::try_from(n).expect("few landings");
        let commit = git
            .write_commit_at(&tree, &parents, &format!("landing {n}"), Some(seconds))
            .expect("commit");
        parent = Some(commit.clone());
        commits.push(commit);
    }
    git.update_ref(
        "refs/heads/master",
        parent.expect("a landing").object(),
        None,
    )
    .expect("master");
    Fixture {
        evidence: Evidence::open(git.git_dir()).trunk("refs/heads/master"),
        dir,
        git,
        landings: commits,
    }
}

pub fn key(name: &str) -> Key {
    Key::parse(name).expect("key")
}

pub fn kind(name: &str) -> Kind {
    Kind::parse(name).expect("kind")
}

pub fn perf() -> Kind {
    kind("perf")
}

/// A measured outcome of one metric.
pub fn measured(metric: &str, value: f64) -> Outcome {
    let mut measure =
        Measure::new(MetricName::parse(metric).expect("metric"), value).expect("finite");
    (measure.n, measure.mad) = (Some(20), Some(1.0));
    Outcome::Measured(vec![measure])
}

pub fn p99(value: f64) -> Outcome {
    measured("block.p99_ns", value)
}

pub fn refused(reason: &str) -> Outcome {
    Outcome::Refused(Reason::new(reason))
}

/// What a run declares about itself.
#[derive(Clone)]
pub struct Shape {
    pub producer: &'static str,
    pub machine: &'static str,
    pub harness: &'static str,
    pub params: Vec<(&'static str, &'static str)>,
    /// The unit and direction the run declares for every metric it measures.
    pub unit: &'static str,
    pub better: Option<Direction>,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            producer: "bench-suite",
            machine: "bench",
            harness: "h/1",
            params: Vec::new(),
            unit: "ns",
            better: Some(Direction::Lower),
        }
    }
}

/// `open` declaring every metric `results` measure, in the shape's unit and direction.
pub fn declaring(mut open: Open, shape: &Shape, results: &[(&str, Outcome)]) -> Open {
    for (_, outcome) in results {
        if let Outcome::Measured(measures) = outcome {
            for measure in measures {
                open.suite
                    .metrics
                    .entry(measure.metric.clone())
                    .or_insert_with(|| Metric::new(shape.unit, shape.better));
            }
        }
    }
    open
}

pub fn open(commit: &CommitId, shape: &Shape, keys: &[&str]) -> Open {
    let mut open = Open::new(
        commit.clone(),
        ProducerName::parse(shape.producer).expect("producer"),
    )
    .harness(shape.harness);
    // As written, so a test can name no machine, which `Machine` refuses.
    shape.machine.clone_into(&mut open.environment.machine);
    open.environment.params = shape
        .params
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    open.keys = Some(keys.iter().map(|k| key(k)).collect());
    open
}

static CLOCK: AtomicU64 = AtomicU64::new(0);

/// A distinct, increasing event time, so runs order by when they were recorded.
pub fn tick() -> String {
    let n = CLOCK.fetch_add(1, Ordering::Relaxed);
    git_process::Timestamp::from_unix_millis(1_790_000_000_000 + u128::from(n) * 1000).into()
}

/// The line at `seq` of `run`, at the next [`tick`].
pub fn event_line(run: &RunId, seq: u32, event: Event) -> EventLine {
    let mut line = EventLine::new(run.clone(), seq, git_process::Timestamp::now(), event);
    line.time = tick();
    line
}

/// The lines of a run: open, the results, and an end unless `end` is `None`.
/// Every metric `results` measure that `open` does not declare is declared in
/// the default shape's unit and direction.
pub fn run_lines(open: Open, results: &[(&str, Outcome)], end: Option<End>) -> Vec<EventLine> {
    let open = declaring(open, &Shape::default(), results);
    let run = RunId::new();
    let mut seq = 0;
    let mut line = |event: Event| {
        let line = event_line(&run, seq, event);
        seq += 1;
        line
    };
    let mut lines = vec![line(Event::Open(Box::new(open)))];
    for (name, outcome) in results {
        lines.push(line(Event::result(key(name), outcome.clone())));
    }
    if let Some(end) = end {
        lines.push(line(Event::close(end)));
    }
    lines
}

/// Records a whole run of `kind` on landing `landing`.
pub fn record_run(
    f: &Fixture,
    kind: &Kind,
    landing: usize,
    shape: &Shape,
    results: &[(&str, Outcome)],
    end: Option<End>,
) -> Recorded {
    let lines = f.run_lines(landing, shape, results, end);
    f.evidence.record(kind, &lines).expect("record")
}

/// The tree of a commit.
pub fn tree(f: &Fixture, commit: &CommitId) -> TreeId {
    TreeId::parse(f.git.tree_of(commit.object()).expect("tree").as_str()).expect("tree")
}

/// Every run on every landing, as `log` lists them: what an incremental
/// catch-up and a rebuild must both answer.
pub fn every_run(at: &At<'_>) -> Result<Vec<git_evidence::LogEntry>, Error> {
    at.log(&git_evidence::LogQuery::Landings(
        git_evidence::Selector::default(),
    ))
}

/// Answers a question at the current pin.
pub fn ask<T>(f: &Fixture, read: impl FnOnce(&At<'_>) -> Result<T, Error>) -> T {
    f.evidence.at(read).expect("answer")
}

/// The name a question was refused for having nothing recorded.
pub fn not_recorded<T: std::fmt::Debug>(
    f: &Fixture,
    read: impl FnOnce(&At<'_>) -> Result<T, Error>,
) -> git_evidence::NotRecorded {
    match f.evidence.at(read) {
        Err(Error::NotRecorded(missing)) => missing,
        other => panic!("expected nothing recorded, got {other:?}"),
    }
}

/// The note of `kind` on `tree`, as Git holds it.
pub fn note(f: &Fixture, kind: &Kind, tree: &TreeId) -> Option<Vec<u8>> {
    git_evidence::testing::note(&f.evidence, kind, tree).expect("read the note")
}

/// The runs of `kind` recorded on the fixture's landings, in note order.
pub fn run_ids(f: &Fixture, kind: &Kind) -> Vec<RunId> {
    let mut ids = Vec::new();
    for landing in &f.landings {
        let Some(text) = note(f, kind, &tree(f, landing)) else {
            continue;
        };
        for line in String::from_utf8_lossy(&text).lines() {
            if let Ok(event) = serde_json::from_str::<EventLine>(line)
                && !ids.contains(&event.run)
            {
                ids.push(event.run);
            }
        }
    }
    ids
}

/// The disposable answer cache, to delete it and compare a rebuild.
pub fn cache(f: &Fixture) -> std::path::PathBuf {
    f.git
        .git_dir()
        .join("evidence")
        .join(git_evidence::testing::index_file_name())
}

/// Appends raw event lines to `landing`'s note of `kind`, bypassing `record`:
/// what a fetch from a faulty store can bring.
pub fn append_raw(f: &Fixture, kind: &Kind, landing: usize, lines: &[EventLine]) {
    let text = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect::<Vec<_>>()
        .join("\n");
    let status = git_process::start(
        git_process::testing::hermetic(&mut Command::new("git"))
            .arg("--git-dir")
            .arg(f.git.git_dir())
            .args(["notes", "--ref"])
            .arg(format!("refs/notes/evidence/{}", kind.as_str()))
            .args(["append", "--no-separator", "-m"])
            .arg(text)
            .arg(tree(f, &f.landings[landing]).as_str()),
    )
    .expect("start git")
    .wait()
    .expect("git");
    assert!(status.success());
}

/// The newest `n` landings.
pub fn newest(n: u32) -> git_evidence::Selector {
    git_evidence::Selector::newest(std::num::NonZeroU32::new(n).expect("a count above zero"))
}

/// The trend of `parse_large`'s `block.p99_ns` over the newest `last`
/// landings.
pub fn parse_large_trend(last: i64) -> git_evidence::TrendQuery {
    let mut query = git_evidence::TrendQuery::new(
        "block.p99_ns".parse().expect("name"),
        "parse_large".parse().expect("name"),
    );
    query.landings = newest(u32::try_from(last).expect("a landing count"));
    query
}

/// A run's end when its producer stopped before answering every key.
pub fn interrupted() -> End {
    End::Refused(git_evidence::Reason::new("interrupted"))
}

/// One perf run on `landing`: `parse_large` measured at `value`, `swap`
/// refused, ended interrupted.
pub fn measured_run(f: &Fixture, landing: usize, value: f64) {
    record_run(
        f,
        &perf(),
        landing,
        &Shape::default(),
        &[("parse_large", p99(value)), ("swap", refused("noisy_host"))],
        Some(interrupted()),
    );
}
