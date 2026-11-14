//! `git-evidence`: the command line, invoked as `git evidence`, `jj evidence`
//! (a jj alias) or `git-evidence`, and the only place that prints. It parses
//! the arguments, asks the Rust API and prints the rows that come back:
//! aligned columns, or with `--csv` CSV. It answers nothing itself, so a Rust
//! caller asking the same question gets every row the command line shows.
//!
//! ```text
//! args ─▶ Evidence (revision ─▶ tree) ─▶ rows ─┬─ aligned columns   (default)
//!                                              └─ CSV               (--csv; always for sql)
//! a name nothing records ─▶ standard error, exit 2
//! ```

mod bench;
mod cli {
    pub mod args;
    pub mod columns;
    pub mod csv;
    pub mod manual;
    pub mod revision;
}

use bench::{Bench, BenchStore, StoreSize};
use clap::Parser as _;
use cli::args::{Cli, Verb};
use cli::revision::{resolve_range, resolve_tree};
use git_evidence::{
    DiffQuery, End, Error, EventLine, Evidence, Key, KeysQuery, Kind, LogQuery, MetricName, Reason,
    RecordError, RemoteError, RunId, Selector, ShowQuery, Shown, TidyRows, TrendQuery,
};
use std::io::{Read as _, Write as _};
use std::num::NonZeroU32;
use std::process::ExitCode;
use std::time::Duration;

/// Landings the overview lists without `--last`.
const OVERVIEW_LANDINGS: NonZeroU32 = NonZeroU32::new(10).expect("10 is not zero");

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            // A name nothing records is exit 2; any other failure exit 1.
            let code = match error.downcast_ref::<Error>() {
                Some(Error::NotRecorded(_)) => 2,
                _ => 1,
            };
            // Nothing useful can be done if stderr itself is closed.
            std::io::stderr()
                .write_all(format!("git-evidence: {error}\n").as_bytes())
                .ok();
            ExitCode::from(code)
        }
    }
}

fn run() -> Result<u8, Box<dyn std::error::Error>> {
    // A malformed command is exit 1, as for any other failure to ask.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = u8::from(error.use_stderr());
            error.print().ok();
            return Ok(code);
        }
    };
    // The manual reads no store.
    if let Some(Verb::Manual { dir }) = &cli.verb {
        cli::manual::write(dir)?;
        return Ok(0);
    }
    // A generated store needs no repository.
    if let Some(Verb::Bench(args)) = &cli.verb {
        return bench(Output::of(&cli), cli.trunk.as_deref(), args);
    }
    let evidence = Evidence::discover(std::env::current_dir()?)?;
    let evidence = match &cli.trunk {
        Some(trunk) => evidence.trunk(trunk.clone()),
        None => evidence,
    };
    let evidence = match cli.time_limit {
        Some(seconds) => evidence.time_limit(Duration::from_secs(u64::from(seconds.get()))),
        None => evidence,
    };
    let output = Output::of(&cli);
    let Some(verb) = cli.verb else {
        evidence.require_complete()?;
        let last = cli.last.unwrap_or(OVERVIEW_LANDINGS);
        return Ok(emit(output, &evidence.at(|at| at.overview(last))?));
    };
    match verb {
        Verb::Audit => {
            let audit = evidence.audit()?;
            emit(output, &audit);
            Ok(if audit.findings.is_empty() { 0 } else { 2 })
        }
        Verb::Fetch { remote } => transfer(output, &evidence, remote, Exchange::Fetch),
        Verb::Push { remote, dry_run } => {
            let how = if dry_run {
                Exchange::DryRun
            } else {
                Exchange::Push
            };
            transfer(output, &evidence, remote, how)
        }
        verb => {
            evidence.require_complete()?;
            ask(output, &evidence, verb)
        }
    }
}

/// Which way evidence travels.
#[derive(Clone, Copy)]
enum Exchange {
    Fetch,
    Push,
    DryRun,
}

fn transfer(
    output: Output,
    evidence: &Evidence,
    remote: Option<String>,
    how: Exchange,
) -> Result<u8, Box<dyn std::error::Error>> {
    let remote = match remote {
        Some(remote) => remote,
        None => evidence.default_remote()?,
    };
    let transfer = match how {
        Exchange::Fetch => evidence.fetch(&remote)?,
        Exchange::DryRun => evidence.push_dry_run(&remote)?,
        Exchange::Push => match evidence.push(&remote) {
            // A refusal, not a fault of the tool: exit 2, as for record.
            Err(
                refused
                @ Error::Remote(RemoteError::PushNotEnabled | RemoteError::AuditFindings(_)),
            ) => {
                std::io::stderr()
                    .write_all(format!("git-evidence: {refused}\n").as_bytes())
                    .ok();
                return Ok(2);
            }
            other => other?,
        },
    };
    emit(output, &transfer);
    Ok(if transfer.refused() { 2 } else { 0 })
}

/// `record`, and every question: one function per verb.
fn ask(output: Output, evidence: &Evidence, verb: Verb) -> Result<u8, Box<dyn std::error::Error>> {
    match verb {
        Verb::Record { kind } => record(output, evidence, &kind),
        Verb::Trend {
            metric,
            key,
            machine,
            range,
            landings,
        } => {
            let landings = selector(evidence, range.as_deref(), &landings)?;
            trend(output, evidence, metric, key, machine, landings)
        }
        Verb::Log { rev, landings } => log(output, evidence, rev, &landings),
        Verb::Show {
            rev,
            kind,
            machine,
            key,
            metric,
        } => {
            let mut query = match rev.parse::<RunId>() {
                Ok(run) => ShowQuery::run(&run),
                Err(_) => ShowQuery::tree(&resolve_tree(evidence, &rev)?),
            };
            (query.kind, query.machine, query.key, query.metric) = (kind, machine, key, metric);
            show(output, evidence, &query)
        }
        Verb::Diff {
            a,
            b,
            kind,
            key,
            metric,
            machine,
        } => {
            let mut query =
                DiffQuery::new(resolve_tree(evidence, &a)?, resolve_tree(evidence, &b)?);
            (query.kind, query.key, query.metric, query.machine) = (kind, key, metric, machine);
            Ok(emit(output, &evidence.at(|at| at.diff(&query))?))
        }
        Verb::Sql { query, path } => sql(evidence, query.as_deref(), path),
        Verb::Keys { kind, metric } => {
            let mut query = KeysQuery::default();
            (query.kind, query.metric) = (kind, metric);
            Ok(emit(output, &evidence.at(|at| at.keys(&query))?))
        }
        Verb::Audit
        | Verb::Fetch { .. }
        | Verb::Push { .. }
        | Verb::Manual { .. }
        | Verb::Bench(_) => {
            unreachable!("answered by run")
        }
    }
}

/// `trend`: one key's metric over the selected landings, the newest
/// [`TrendQuery::DEFAULT_LAST`] when nothing narrows them.
fn trend(
    output: Output,
    evidence: &Evidence,
    metric: MetricName,
    key: Key,
    machine: Option<String>,
    mut landings: Selector,
) -> Result<u8, Box<dyn std::error::Error>> {
    if landings.is_everything() {
        landings.last = Some(TrendQuery::DEFAULT_LAST);
    }
    let mut query = TrendQuery::new(metric, key);
    (query.machine, query.landings) = (machine, landings);
    Ok(emit(output, &evidence.at(|at| at.trend(&query))?))
}

/// `show RUN` prints the run with its parameters; `show REV` every run's
/// outcome rows.
fn show(
    output: Output,
    evidence: &Evidence,
    query: &ShowQuery,
) -> Result<u8, Box<dyn std::error::Error>> {
    let runs = evidence.at(|at| at.show(query))?;
    Ok(match (&query.shown, runs.as_slice()) {
        (Shown::Run(_), [run]) => emit(output, run),
        _ => emit(output, &runs),
    })
}

/// `log REV` (one tree) or `log A..B` and the selector flags (landings).
fn log(
    output: Output,
    evidence: &Evidence,
    rev: Option<String>,
    landings: &cli::args::SelectorArgs,
) -> Result<u8, Box<dyn std::error::Error>> {
    let selected = |range| selector(evidence, range, landings);
    let query = match rev {
        Some(rev) if !rev.contains("..") => {
            if !selected(None)?.is_everything() {
                return Err(
                    "a revision names one tree; select landings with A..B, --last, --since or --until".into(),
                );
            }
            LogQuery::tree(&resolve_tree(evidence, &rev)?)
        }
        range => LogQuery::landings(selected(range.as_deref())?),
    };
    Ok(emit(output, &evidence.at(|at| at.log(&query))?))
}

/// `bench`: the figures as text, JSON or CSV rows; with `--returned`, also
/// one run of kind `tool` in a file, opened before the work and closed
/// refused `bench_failed` if the bench cannot measure.
fn bench(
    output: Output,
    trunk: Option<&str>,
    args: &cli::args::Bench,
) -> Result<u8, Box<dyn std::error::Error>> {
    let evidence = if args.this_store || args.returned.is_some() {
        let evidence = Evidence::discover(std::env::current_dir()?)?;
        Some(match trunk {
            Some(trunk) => evidence.trunk(trunk),
            None => evidence,
        })
    } else {
        None
    };
    let store = match (&evidence, args.this_store) {
        (Some(evidence), true) => BenchStore::Existing(evidence),
        _ => BenchStore::Generated(StoreSize {
            landings: args.landings,
            runs: args.runs,
        }),
    };
    let bench = Bench {
        store,
        repeats: args.repeats,
    };
    let returned = match (&evidence, &args.returned, &args.on) {
        (Some(evidence), Some(path), Some(on)) => {
            let commit = cli::revision::resolve_commit(evidence, on)?;
            let open = bench.open(commit, &evidence.machine()?);
            Some(evidence.run_to_file(&bench::bench_kind(), open, path)?)
        }
        _ => None,
    };
    let measured = bench
        .run(&std::env::current_exe()?)
        .and_then(|report| report.outcomes().map(|outcomes| (report, outcomes)));
    let (report, outcomes) = match measured {
        Ok(measured) => measured,
        Err(error) => {
            if let Some(run) = returned {
                run.finish().end(End::Refused(
                    Reason::new("bench_failed").detail(error.to_string()),
                ))?;
            }
            return Err(error.into());
        }
    };
    if let Some(run) = returned {
        run.finish().results(outcomes).end(End::Complete)?;
    }
    Ok(emit(output, &report))
}

/// The landings a question reads: `A..B` and the selector flags.
fn selector(
    evidence: &Evidence,
    range: Option<&str>,
    landings: &cli::args::SelectorArgs,
) -> Result<Selector, Box<dyn std::error::Error>> {
    let range = match range {
        Some(range) => Some(
            resolve_range(evidence, range)?
                .ok_or_else(|| format!("{range} is not a range: write A..B"))?,
        ),
        None => None,
    };
    let mut selector = Selector::default();
    (
        selector.range,
        selector.since,
        selector.until,
        selector.last,
    ) = (range, landings.since, landings.until, landings.last);
    Ok(selector)
}

/// `sql`: the rows as CSV, a NULL as an empty field; with `--path`, the
/// refreshed index file's path.
fn sql(
    evidence: &Evidence,
    query: Option<&str>,
    path: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let (Some(query), false) = (query, path) else {
        print(&format!("{}\n", evidence.sql_path()?.display()));
        return Ok(0);
    };
    let rows = evidence.sql(query)?;
    let cells: Vec<Vec<Option<String>>> = rows
        .rows
        .iter()
        .map(|row| row.iter().map(git_evidence::SqlValue::cell).collect())
        .collect();
    print(&cli::csv::table(&rows.columns, &cells));
    Ok(0)
}

/// Event lines from standard input, recorded. A refusal is an answer about
/// the input, not a fault of the tool: exit 2.
fn record(
    output: Output,
    evidence: &Evidence,
    kind: &Kind,
) -> Result<u8, Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let refuse = |why: &str| {
        std::io::stderr()
            .write_all(format!("git-evidence: refused: {why}\n").as_bytes())
            .ok();
        Ok(2)
    };
    let lines = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(number, line)| {
            serde_json::from_str::<EventLine>(line)
                .map_err(|error| format!("line {}: {error}", number + 1))
        })
        .collect::<Result<Vec<_>, _>>();
    let lines = match lines {
        Ok(lines) => lines,
        Err(why) => return refuse(&why),
    };
    // A caller that piped nothing (a producer that wrote no artifact) learns
    // it here, by the exit status, rather than by reading an empty table.
    if lines.is_empty() {
        return refuse("the input holds no event line; nothing is recorded");
    }
    match evidence.record(kind, &lines) {
        Ok(report) => Ok(emit(output, &report)),
        Err(Error::Record(RecordError::Refused(why))) => refuse(&why.to_string()),
        Err(error) => Err(error.into()),
    }
}

/// How the command line prints rows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Output {
    /// Aligned columns, for a terminal.
    Columns,
    /// CSV, for a script.
    Csv,
}

impl Output {
    const fn of(cli: &Cli) -> Self {
        if cli.csv { Self::Csv } else { Self::Columns }
    }
}

/// Prints `value`'s rows under its header; exit 0.
fn emit<T: TidyRows>(output: Output, value: &T) -> u8 {
    let (header, rows) = (value.header(), value.rows());
    print(&match output {
        Output::Columns => cli::columns::table(&header, &rows),
        Output::Csv => cli::csv::table(&header, &rows),
    });
    0
}

fn print(text: &str) {
    // A closed stdout (a pipe to `head`) is not an error worth reporting.
    std::io::stdout().write_all(text.as_bytes()).ok();
}
