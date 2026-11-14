//! A bench run: the store made, the binary timed on it, cold then warm, per
//! verb, each figure the wall time of one process (README "Benchmark").
//! Each timed invocation retries only before it starts. Those waits are inside
//! the clock; declared warm repetitions remain separate invocations.

use super::figures::{
    BenchFigure, BenchOrigin, BenchReport, BenchSample, BenchTemperature, BenchTiming, BenchVerb,
};
use super::generate;
use super::store::{self, BenchStore};
use git_evidence::NotFinite;
use git_evidence::SqlValue;
use git_evidence::StoreError;
use git_evidence::{CommitId, IdError};
use git_evidence::{Error, Evidence};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// The `sql` verb's reference query: measures per kind over the newest 30
/// landings, the shape of the README's examples.
const REFERENCE_SQL: &str = "SELECT r.kind, count(*) AS measures FROM runs r JOIN measures m USING (run) \
     JOIN landings l USING (landing) WHERE l.landing > (SELECT max(landing) - 30 FROM landings) \
     GROUP BY r.kind";

/// One bench: a store, and how many times each warm figure is taken.
#[derive(Debug, Clone, Copy)]
pub struct Bench<'a> {
    pub store: BenchStore<'a>,
    pub repeats: NonZeroU32,
}

/// Why a bench could not measure.
#[derive(Debug, thiserror::Error)]
pub enum BenchError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Evidence(#[from] Error),
    #[error(transparent)]
    Name(#[from] IdError),
    #[error(transparent)]
    NotFinite(#[from] NotFinite),
    #[error("the bench store answers no {0}: {1}")]
    Unreadable(&'static str, String),
}

impl From<git_process::GitError> for BenchError {
    fn from(error: git_process::GitError) -> Self {
        Self::Store(StoreError::from(error))
    }
}

impl Bench<'_> {
    /// Times `program` (a `git-evidence` binary) on this bench's store.
    ///
    /// # Errors
    /// The store cannot be made (a temporary directory, Git), or the
    /// measured store cannot be counted. A command that fails is a figure
    /// ([`BenchFigure::Failed`]), not an error.
    pub fn run(&self, program: &Path) -> Result<BenchReport, BenchError> {
        let dir = tempfile::Builder::new()
            .prefix("git-evidence-bench-")
            .tempdir()
            .map_err(|source| StoreError::Io {
                path: std::env::temp_dir(),
                source,
            })?;
        let git = store::init_bare(&dir.path().join("store.git"))?;
        let (origin, trunk) = match self.store {
            BenchStore::Generated(size) => {
                generate::generate(&git, size)?;
                (BenchOrigin::Generated, generate::TRUNK.to_owned())
            }
            BenchStore::Existing(evidence) => (BenchOrigin::Existing, store::copy(evidence, &git)?),
        };
        let target = Target {
            program,
            git_dir: git.git_dir().to_path_buf(),
            trunk: trunk.clone(),
            repeats: self.repeats.get(),
        };
        let mut timings = vec![BenchTiming {
            verb: BenchVerb::Overview,
            temperature: BenchTemperature::Cold,
            took: target.once(&[], None),
        }];
        let evidence = Evidence::open(git.git_dir()).trunk(trunk.clone());
        let followed = followed(&evidence);
        let questions = [
            (BenchVerb::Overview, Ok(vec![])),
            (
                BenchVerb::Trend,
                followed
                    .as_ref()
                    .map(|(metric, key)| vec!["trend", metric.as_str(), key.as_str()])
                    .map_err(Clone::clone),
            ),
            (BenchVerb::Sql, Ok(vec!["sql", REFERENCE_SQL])),
        ];
        for (verb, args) in questions {
            let took = match args {
                Ok(args) => target.warm(&args, || Ok(None)),
                Err(why) => BenchFigure::Failed(why),
            };
            timings.push(warm(verb, took));
        }
        let (landings, runs) = size(&evidence)?;
        let tip = git
            .resolve(&format!("{trunk}^{{commit}}"))?
            .ok_or_else(|| BenchError::Unreadable("trunk", trunk.clone()))?;
        let tip = CommitId::parse(tip.as_str())?;
        let record = target.warm(&["record", "perf"], || {
            record_lines(&tip)
                .map(Some)
                .map_err(|e| format!("no run to record: {e}"))
        });
        timings.push(warm(BenchVerb::Record, record));
        Ok(BenchReport {
            origin,
            landings,
            runs,
            repeats: self.repeats.get(),
            timings,
        })
    }
}

fn warm(verb: BenchVerb, took: BenchFigure) -> BenchTiming {
    BenchTiming {
        verb,
        temperature: BenchTemperature::Warm,
        took,
    }
}

/// A store being measured, and the binary measuring it.
struct Target<'a> {
    program: &'a Path,
    git_dir: PathBuf,
    trunk: String,
    repeats: u32,
}

impl Target<'_> {
    /// One timed answer of `args`.
    fn once(&self, args: &[&str], stdin: Option<Vec<u8>>) -> BenchFigure {
        match self.time(args, stdin) {
            Ok(ms) => BenchFigure::Measured(BenchSample {
                median_ms: ms,
                mad_ms: 0.0,
                n: 1,
            }),
            Err(why) => BenchFigure::Failed(why),
        }
    }

    /// `args` once untimed, then timed `repeats` times, each with the input
    /// `stdin` makes for it.
    fn warm(
        &self,
        args: &[&str],
        stdin: impl Fn() -> Result<Option<Vec<u8>>, String>,
    ) -> BenchFigure {
        if let Err(why) = stdin().and_then(|input| self.time(args, input)) {
            return BenchFigure::Failed(why);
        }
        let mut times = Vec::new();
        for _ in 0..self.repeats {
            match stdin().and_then(|input| self.time(args, input)) {
                Ok(ms) => times.push(ms),
                Err(why) => return BenchFigure::Failed(why),
            }
        }
        let (median_ms, mad_ms) = super::stats::median_and_mad(&times);
        BenchFigure::Measured(BenchSample {
            median_ms,
            mad_ms,
            n: self.repeats,
        })
    }

    /// The wall time of `program --trunk <trunk> args` on the store, in
    /// milliseconds. Any answer counts (exit 0 or 2; for `record`, only 0);
    /// anything else is a failure, named by the last line it printed on
    /// standard error.
    fn time(&self, args: &[&str], stdin: Option<Vec<u8>>) -> Result<f64, String> {
        let mut command = Command::new(self.program);
        git_process::isolated(&mut command)
            .current_dir(self.git_dir.parent().unwrap_or(&self.git_dir))
            .env("GIT_EVIDENCE_GIT_DIR", &self.git_dir)
            .arg("--trunk")
            .arg(&self.trunk)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let mut child = git_process::start(&mut command).map_err(|error| {
            format!(
                "{} did not start: {}",
                self.program.display(),
                std::error::Error::source(&error).unwrap_or(&error)
            )
        })?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            use std::io::Write as _;
            pipe.write_all(&bytes)
                .map_err(|e| format!("could not feed {}: {e}", args.join(" ")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("{} did not finish: {e}", args.join(" ")))?;
        let took = started.elapsed().as_secs_f64() * 1000.0;
        // Printed rows (0) and a name nothing records (2) are both answers; a
        // refused `record` (exit 2) wrote no run, so it is no figure.
        let answered = match output.status.code() {
            Some(0) => true,
            Some(2) => args.first() != Some(&"record"),
            _ => false,
        };
        match output.status.code() {
            _ if answered => Ok(took),
            code => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
                Err(format!(
                    "`git evidence {}` exited {}: {}",
                    if args.is_empty() { "" } else { args[0] },
                    code.map_or_else(|| "by a signal".to_owned(), |c| c.to_string()),
                    last.unwrap_or("nothing on standard error")
                ))
            }
        }
    }
}

/// The metric and key `trend` follows: the first, by name, that the newest
/// landing's runs measured; untimed, after the cold figure.
fn followed(evidence: &Evidence) -> Result<(String, String), String> {
    let rows = evidence
        .sql(
            "SELECT m.metric, m.key FROM measures m JOIN runs r USING (run) \
             WHERE r.landing = (SELECT max(landing) FROM runs) ORDER BY m.metric, m.key LIMIT 1",
        )
        .map_err(|e| format!("no metric to follow: {e}"))?;
    match rows.rows.first().map(Vec::as_slice) {
        Some([SqlValue::Text(metric), SqlValue::Text(key)]) => Ok((metric.clone(), key.clone())),
        _ => Err("no metric to follow: the newest landing's runs measured nothing".into()),
    }
}

/// The measured store's landings and runs.
fn size(evidence: &Evidence) -> Result<(u64, u64), BenchError> {
    let rows = evidence
        .sql("SELECT (SELECT count(*) FROM landings), (SELECT count(*) FROM runs)")
        .map_err(|e| BenchError::Unreadable("size", e.to_string()))?;
    match rows.rows.first().map(Vec::as_slice) {
        Some(&[SqlValue::Integer(landings), SqlValue::Integer(runs)]) => {
            let count = |n: i64| {
                u64::try_from(n).map_err(|_| BenchError::Unreadable("size", n.to_string()))
            };
            Ok((count(landings)?, count(runs)?))
        }
        other => Err(BenchError::Unreadable("size", format!("{other:?}"))),
    }
}

/// A fresh perf run on `commit`, as event lines for `record`: 16 measured
/// keys, the shape of the generated store's perf runs, under a new run id.
fn record_lines(commit: &CommitId) -> Result<Vec<u8>, BenchError> {
    let mut bytes = Vec::new();
    for line in generate::fresh_run(commit)? {
        bytes.extend(
            serde_json::to_vec(&line).map_err(|e| BenchError::Unreadable("run", e.to_string()))?,
        );
        bytes.push(b'\n');
    }
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_measured_executable_start_reaches_the_observer_and_keeps_its_failure() {
        const CHILD: &str = "GIT_EVIDENCE_BENCH_START_PROBE";
        if let Some(directory) = std::env::var_os(CHILD) {
            let directory = std::path::Path::new(&directory);
            struct ObservedStart;
            impl git_process::StartObserver for ObservedStart {
                fn failed(record: &git_process::StartFailureRecord) {
                    let directory = std::env::var_os(CHILD).expect("probe directory");
                    std::fs::write(
                        std::path::Path::new(&directory).join("start.json"),
                        serde_json::to_vec(record).expect("typed start"),
                    )
                    .expect("observed start");
                }
            }
            git_process::observe_start_failures::<ObservedStart>().expect("probe observer");
            let program = directory.join("invalid-tool");
            let target = Target {
                program: &program,
                git_dir: directory.to_owned(),
                trunk: "master".into(),
                repeats: 1,
            };
            let failure = target
                .time(&[], None)
                .expect_err("absent executable never runs");
            assert!(failure.contains("did not start"));
            std::fs::write(directory.join("failure"), failure).expect("original failure");
            return;
        }
        let directory = tempfile::tempdir().expect("private probe");
        let program = directory.path().join("invalid-tool");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "bench::measure::tests::a_measured_executable_start_reaches_the_observer_and_keeps_its_failure", "--nocapture"])
            .env(CHILD, directory.path())
            .output().expect("probe child");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records = std::fs::read_dir(directory.path())
            .expect("probe outputs")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .map(|path| {
                serde_json::from_slice::<git_process::StartFailureRecord>(
                    &std::fs::read(path).expect("notification"),
                )
                .expect("typed notification")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            records.len(),
            1,
            "the timed command must use the observed start boundary"
        );
        assert_eq!(records[0].attempt, 1);
        assert!(!records[0].retrying);
        assert_eq!(records[0].program, program.to_string_lossy());
        let native = std::io::Error::from_raw_os_error(records[0].os_error.expect("native error"))
            .to_string();
        assert!(
            std::fs::read_to_string(directory.path().join("failure"))
                .expect("failure text")
                .contains(&native)
        );
    }
}
