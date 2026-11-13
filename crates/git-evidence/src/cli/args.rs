//! The command line's grammar: the verbs and their options, as `--help`
//! prints them (README "Reading" and "Writing").

use clap::{Args, Parser, Subcommand};
use git_evidence::{Key, Kind, Machine, MetricName, UtcDate};

#[derive(Parser)]
#[command(
    name = "git-evidence",
    version,
    about = "Numbers recorded per Git commit, kept in Git notes",
    after_long_help = super::manual::after_help()
)]
pub struct Cli {
    /// Print the rows as CSV under the same header, instead of aligned columns.
    #[arg(long, global = true)]
    pub csv: bool,
    /// The ref whose first-parent line is the landings. Default: origin/HEAD's
    /// target, else origin/main, origin/master, main, master.
    #[arg(long, global = true)]
    pub trunk: Option<String>,
    /// Stop any read that runs longer than SECONDS, releasing its snapshot of
    /// the index (default 30).
    #[arg(long, global = true, value_name = "SECONDS")]
    pub time_limit: Option<std::num::NonZeroU32>,
    /// Without a verb: how many of the newest landings to list (default 10).
    #[arg(long, value_name = "N")]
    pub last: Option<std::num::NonZeroU32>,
    /// Without a verb: the newest landings, each with its run count of each
    /// kind and its subject.
    #[command(subcommand)]
    pub verb: Option<Verb>,
}

#[derive(Subcommand)]
pub enum Verb {
    /// One metric of one key over the landings: one row per run.
    Trend {
        /// The metric, e.g. latency.p99_ns (`keys` lists them).
        metric: MetricName,
        /// The key, e.g. parser/parse_large.
        key: Key,
        /// Only this machine's runs.
        #[arg(long)]
        machine: Option<Machine>,
        /// A..B: the landings after A, through B. Without any selection: --last 30.
        range: Option<String>,
        #[command(flatten)]
        landings: SelectorArgs,
    },
    /// One row per run on a revision's tree, or on each landing of A..B or a selection.
    Log {
        /// A revision (see REV in the manual), or A..B.
        #[arg(required_unless_present_any = ["last", "since", "until"])]
        rev: Option<String>,
        #[command(flatten)]
        landings: SelectorArgs,
    },
    /// The numbers that differ between two revisions, side by side, with the change.
    Diff {
        /// The earlier revision (see REV in the manual).
        a: String,
        /// The later revision.
        b: String,
        #[arg(long)]
        kind: Option<Kind>,
        #[arg(long)]
        key: Option<Key>,
        /// A glob over metric names.
        #[arg(long)]
        metric: Option<String>,
        #[arg(long)]
        machine: Option<Machine>,
    },
    /// One row per number or outcome recorded on a revision, or in one run.
    Show {
        /// A revision, or a run id (a ULID): one run, its parameters first.
        rev: String,
        /// Only runs whose kind matches this glob.
        #[arg(long, value_name = "GLOB")]
        kind: Option<String>,
        /// Only runs whose machine matches this glob.
        #[arg(long, value_name = "GLOB")]
        machine: Option<String>,
        /// Only keys matching this glob.
        #[arg(long, value_name = "GLOB")]
        key: Option<String>,
        /// Only metrics matching this glob (an outcome's metric is its kind:
        /// verdict, refused, skipped).
        #[arg(long, value_name = "GLOB")]
        metric: Option<String>,
    },
    /// The keys and metrics recorded, one row each.
    Keys {
        #[arg(long)]
        kind: Option<Kind>,
        /// A glob over metric names.
        #[arg(long)]
        metric: Option<String>,
    },
    /// Run one read-only SQL query on the refreshed index; the rows as CSV.
    ///
    /// The public views are listed below and in the README.
    #[command(after_long_help = super::manual::sql_help())]
    Sql {
        /// The query: one statement that reads rows. It may open with a `--`
        /// comment line.
        #[arg(
            required_unless_present = "path",
            conflicts_with = "path",
            allow_hyphen_values = true
        )]
        query: Option<String>,
        /// Refresh the index and print its file's path, for sqlite3 or DuckDB.
        #[arg(long)]
        path: bool,
    },
    /// Record event lines (JSON, one per line) of one kind from standard input.
    Record { kind: Kind },
    /// Time this binary on a store nobody else reads: cold (the overview
    /// from no index) and warm (overview, trend, sql, record), wall time per
    /// process. Default: a generated store of 5,000 landings and 10,000 runs.
    Bench(Bench),
    /// Fetch every kind's evidence and merge it into this store's. Never forces.
    Fetch {
        /// A configured remote. Default: Git config evidence.remote, else origin.
        remote: Option<String>,
    },
    /// What a push would publish: each ref's size, and anything that does not
    /// read or looks personal. Exits 2 while there are findings.
    Audit,
    /// Push this store's evidence; merge and retry if another store pushed first.
    /// Off until `git config --local evidence.push true`.
    Push {
        /// A configured remote. Default: Git config evidence.remote, else origin.
        remote: Option<String>,
        /// Show what would be pushed, and send nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Write the manual pages into DIR, for the installer.
    #[command(hide = true)]
    Manual { dir: std::path::PathBuf },
}

/// What `bench` measures, and where its run goes.
#[derive(Args)]
pub struct Bench {
    /// Measure a copy of this store (its evidence and trunk, sharing its
    /// objects) instead of a generated one.
    #[arg(long, conflicts_with_all = ["landings", "runs"])]
    pub this_store: bool,
    /// Landings of the generated store.
    #[arg(long, default_value_t = git_evidence::StoreSize::TARGET.landings)]
    pub landings: std::num::NonZeroU32,
    /// Runs of the generated store, spread evenly over its landings.
    #[arg(long, default_value_t = git_evidence::StoreSize::TARGET.runs)]
    pub runs: u32,
    /// Times each warm figure is taken; it reports their median.
    #[arg(long, default_value = "5")]
    pub repeats: std::num::NonZeroU32,
    /// Also write the figures to PATH as one run of kind `tool` about the
    /// revision --on names, opened before the work, for `git evidence record
    /// tool < PATH`. The machine is this store's evidence.machine.
    #[arg(long, value_name = "PATH", requires = "on")]
    pub returned: Option<std::path::PathBuf>,
    /// The revision whose git-evidence is being measured (with --returned).
    #[arg(long, value_name = "REV", requires = "returned")]
    pub on: Option<String>,
}

/// The landing selector every question over landings shares (`SELECT` in
/// README "Reading"): with a range `A..B`, each narrows the trunk.
#[derive(Args)]
pub struct SelectorArgs {
    /// Only the newest N landings of the selection.
    #[arg(long)]
    pub last: Option<std::num::NonZeroU32>,
    /// Only landings committed on or after DATE: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ, UTC.
    #[arg(long, value_name = "DATE")]
    pub since: Option<UtcDate>,
    /// Only landings committed on or before DATE (a whole day, or to the second).
    #[arg(long, value_name = "DATE")]
    pub until: Option<UtcDate>,
}
