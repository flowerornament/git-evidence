//! The one entry point: [`Evidence`], a repository's evidence.
//!
//! ```text
//! Evidence::discover(path) ─┬─ .machine()               the declared machine (git config evidence.machine)
//!                           ├─ .run(kind, Open) → Run   write: declare · result · results · finish · close
//!                           ├─ .resume(kind, &RunId)    after a crash
//!                           ├─ .record(kind, &[lines])  lines built elsewhere (imports, other languages)
//!                           ├─ .at(|at| at.trend(&q))   read: questions at one pin, a query each
//!                           ├─ .sql(query) → Rows       rows of the public views
//!                           └─ .fetch(remote) · .push(remote)
//! ```
//!
//! Every question returns rows of recorded numbers, an empty list when none
//! match, or [`NotRecorded`] when what it names has nothing recorded. No method
//! hands out a database handle, and none hands out a Git writer. So for a
//! caller that uses this API, notes change only through [`Evidence::record`]
//! and its rules. That is a
//! property of the API, not of the repository: a caller that opens
//! `git_process::Git` at [`Evidence::git_dir`], to read history beside the
//! evidence, can also write anything there, as any program running `git` can.
//!
//! `Evidence` has no Git accessor:
//!
//! ```compile_fail
//! fn handle(evidence: &git_evidence::Evidence) {
//!     let _ = evidence.git();
//! }
//! ```

use crate::ask::sql::{Rows, SqlError};
use crate::ask::{
    self, DiffQuery, Difference, KeyInfo, KeysQuery, LogEntry, LogQuery, NotRecorded, Overview,
    Point, RunDetail, ShowQuery, TrendQuery,
};
use crate::format::event::{EventLine, Open};
use crate::format::id::{IdError, Kind, Machine, RunId};
use crate::index::{Index, IndexError, Pin, Runs};
use crate::share::audit::Audit;
use crate::share::remote::{RemoteError, Transfer};
use crate::store::{Store, StoreError};
use crate::write::{RecordError, Recorded, Resumed, Run, record};
use rusqlite::Connection;
use std::num::NonZeroU32;
use std::path::Path;

/// What can go wrong using [`Evidence`]. A refusal is [`RecordError::Refused`],
/// naming the rule and its remedy.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error(transparent)]
    Name(#[from] IdError),
    #[error(transparent)]
    Sql(#[from] SqlError),
    /// A question named something the store has nothing recorded for.
    #[error(transparent)]
    NotRecorded(#[from] NotRecorded),
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Index(IndexError::Sql(error))
    }
}

/// A repository's evidence.
#[derive(Debug, Clone)]
pub struct Evidence {
    store: Store,
    trunk: Option<String>,
    time_limit: std::time::Duration,
}

impl Evidence {
    /// The evidence of the repository containing `path` (jj workspaces too).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] outside a repository.
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            store: Store::discover(path.as_ref())?,
            trunk: None,
            time_limit: crate::index::TIME_LIMIT,
        })
    }

    /// The evidence of the store with this Git directory, without discovery:
    /// for a caller that already knows it (a bare repository, a test's
    /// scratch store). Nothing is read until a method needs it.
    #[must_use]
    pub fn open(git_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            store: Store::open(git_dir),
            trunk: None,
            time_limit: crate::index::TIME_LIMIT,
        }
    }

    /// Whether [`Evidence::discover`] found this store through a jj workspace,
    /// whose revisions are jj's to name, never the shared store's Git refs.
    #[must_use]
    pub fn in_jj_workspace(&self) -> bool {
        self.store.in_jj_workspace()
    }

    /// Reads landings from `trunk` (a ref) instead of the default trunk.
    #[must_use]
    pub fn trunk(mut self, trunk: impl Into<String>) -> Self {
        self.trunk = Some(trunk.into());
        self
    }

    /// Stops any read (an answer, or `sql`) that runs longer than `limit`,
    /// instead of the default 30 s: a read holds a snapshot of the index,
    /// and while it does, the index's write-ahead log cannot be reset. A
    /// read past its limit is refused with [`IndexError::TimeLimit`].
    #[must_use]
    pub const fn time_limit(mut self, limit: std::time::Duration) -> Self {
        self.time_limit = limit;
        self
    }

    /// The store's Git directory (the shared one, for a worktree or a jj
    /// workspace), for tools that read its history beside the evidence: an
    /// importer walking a file's versions on the trunk. Writing through it
    /// bypasses every rule `record` keeps.
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        self.store.git().git_dir()
    }

    #[cfg(feature = "testing")]
    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    #[cfg(feature = "testing")]
    pub(crate) fn git(&self) -> &git_process::Git {
        self.store.git()
    }

    /// Refuses a shallow or partial clone, whose history or note blobs are
    /// missing, before a tool spends work that would be refused later.
    ///
    /// # Errors
    /// [`StoreError::Incomplete`], or a Git failure.
    pub fn require_complete(&self) -> Result<(), Error> {
        Ok(self.store.require_complete()?)
    }

    /// The trunk landings are read from: the one set by [`Evidence::trunk`],
    /// else the store's default.
    ///
    /// # Errors
    /// [`StoreError::NoTrunk`] when there is none.
    pub fn trunk_ref(&self) -> Result<String, Error> {
        match &self.trunk {
            Some(trunk) => Ok(trunk.clone()),
            None => Ok(self.store.default_trunk()?),
        }
    }

    /// The machine this store's producers run on: Git config `evidence.machine`.
    ///
    /// # Errors
    /// Missing or invalid, naming `git config evidence.machine <name>`.
    pub fn machine(&self) -> Result<Machine, Error> {
        Ok(self.store.machine()?)
    }

    /// The size budget of one run of `kind` in this store, in bytes:
    /// `evidence.<kind>.budget`, `None` while unset. A producer that writes for
    /// this store elsewhere trims to it.
    ///
    /// # Errors
    /// A value that is not a byte count, or that Git cannot read (never read
    /// as "no budget").
    pub fn budget(&self, kind: &Kind) -> Result<Option<u64>, Error> {
        Ok(self.store.budget(kind)?)
    }

    /// Opens a run of `kind`: durable before any work starts.
    ///
    /// # Errors
    /// A refusal of the open (an unknown commit, a redefined metric); a Git
    /// failure.
    pub fn run(&self, kind: &Kind, open: Open) -> Result<Run, Error> {
        Ok(Run::open(&self.store, kind, open)?)
    }

    /// Opens a run of `kind` whose lines go to the file at `path` instead of
    /// this store, for the store of whoever asked for the run to record: a
    /// measuring host that holds no evidence. Every write is checked against
    /// this store (the commit, the run grammar, the kind's budget) as
    /// [`Evidence::record_dry_run`] would, then replaces the file whole, so the file is
    /// always one run, durable from its open on.
    ///
    /// # Errors
    /// As [`Evidence::run`], and a file that cannot be written.
    pub fn run_to_file(
        &self,
        kind: &Kind,
        open: Open,
        path: impl Into<std::path::PathBuf>,
    ) -> Result<Run, Error> {
        Ok(Run::open_to_file(&self.store, kind, open, path.into())?)
    }

    /// Finds an unfinished run this producer opened. Only the producer that
    /// opened a run closes it.
    ///
    /// # Errors
    /// A run that is not recorded or has ended is refused.
    pub fn resume(&self, kind: &Kind, id: &RunId) -> Result<Resumed, Error> {
        let mut index = self.locator()?;
        index.catch_up_runs()?;
        Ok(Run::resume(&self.store, kind, id, &index)?)
    }

    /// Records event lines built elsewhere: a whole run, or the next events of
    /// one already recorded.
    ///
    /// # Errors
    /// A refused batch writes nothing; the refusal names its rule and remedy.
    pub fn record(&self, kind: &Kind, lines: &[EventLine]) -> Result<Recorded, Error> {
        let mut index = self.locator()?;
        index.catch_up_runs()?;
        Ok(record(&self.store, kind, lines, &index)?)
    }

    /// What [`Evidence::record`] would do with `lines`, without writing: every
    /// refusal it would give, the bytes added, and each run's size after it
    /// (to compare with `evidence.<kind>.budget`).
    ///
    /// # Errors
    /// As [`Evidence::record`].
    pub fn record_dry_run(&self, kind: &Kind, lines: &[EventLine]) -> Result<Recorded, Error> {
        let mut index = self.locator()?;
        index.catch_up_runs()?;
        Ok(crate::write::record_dry_run(
            &self.store,
            kind,
            lines,
            &index,
        )?)
    }

    /// The questions, answered at one pin in one read snapshot: each verb is
    /// a method of [`At`] taking its query, so one question is
    /// `evidence.at(|at| at.trend(&query))`. The questions never hold a lock
    /// other processes wait on; asking `Evidence` again inside them works, at
    /// a pin of its own.
    ///
    /// # Errors
    /// What `questions` returns, or an index failure.
    pub fn at<T>(&self, questions: impl FnOnce(&At<'_>) -> Result<T, Error>) -> Result<T, Error> {
        let mut index = self.index()?;
        let mut outcome = None;
        index.read(|ix, pin| {
            outcome = Some(questions(&At { db: ix.db(), pin }));
            Ok(())
        })?;
        outcome.expect("the transaction ran the questions")
    }

    /// Brings the index up to date with the notes and the trunk, then runs
    /// `query` read-only against it: the public views (README "SQL") or any
    /// table. The rows are the recorded numbers: nothing here computes a
    /// verdict.
    ///
    /// # Errors
    /// [`SqlError`]: a statement that writes or returns no rows, duplicate
    /// column names, SQLite's own refusal; an index failure.
    pub fn sql(&self, query: &str) -> Result<Rows, Error> {
        let mut index = self.index()?;
        Ok(crate::ask::sql::query(&mut index, query)?)
    }

    /// Brings the index up to date and returns its file, for `sqlite3`, DuckDB or
    /// any tool that opens it itself. The file is a
    /// cache: only the public views are stable, and only until the next
    /// refresh is it current.
    ///
    /// # Errors
    /// An index failure.
    pub fn sql_path(&self) -> Result<std::path::PathBuf, Error> {
        let mut index = self.index()?;
        index.catch_up()?;
        Ok(index.path().to_owned())
    }

    /// Fetches the remote's evidence and merges it by set union.
    ///
    /// # Errors
    /// A remote that is not a configured name; a Git failure.
    pub fn fetch(&self, remote: &str) -> Result<Transfer, Error> {
        Ok(crate::share::remote::fetch(&self.store, remote)?)
    }

    /// Pushes what the remote lacks of this store's evidence; fetches and
    /// merges first when the remote holds evidence this store lacks. What is
    /// sent is audited first: nothing is pushed while it has a finding.
    ///
    /// # Errors
    /// As [`Evidence::fetch`]; pushing not enabled; an audit with findings.
    pub fn push(&self, remote: &str) -> Result<Transfer, Error> {
        Ok(crate::share::remote::push(&self.store, remote)?)
    }

    /// What [`Evidence::push`] would send, without sending or merging anything.
    ///
    /// # Errors
    /// As [`Evidence::fetch`].
    pub fn push_dry_run(&self, remote: &str) -> Result<Transfer, Error> {
        Ok(crate::share::remote::push_dry_run(&self.store, remote)?)
    }

    /// The remote evidence travels through: Git config `evidence.remote`, else `origin`.
    ///
    /// # Errors
    /// A Git failure.
    pub fn default_remote(&self) -> Result<String, Error> {
        Ok(crate::share::remote::default_remote(&self.store)?)
    }

    /// What a push would publish: each ref's size, the notes commits'
    /// identities, and anything that does not fold or looks personal.
    ///
    /// # Errors
    /// As [`Evidence::fetch`]: a shallow or partial clone is refused, since it
    /// cannot see everything a push would send.
    pub fn audit(&self) -> Result<Audit, Error> {
        Ok(crate::share::audit::audit(&self.store)?)
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn push_against(
        &self,
        remote: &str,
        own: &crate::share::audit::OwnNames,
    ) -> Result<Transfer, Error> {
        Ok(crate::share::remote::push_against(
            &self.store,
            remote,
            own,
        )?)
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn audit_against(
        &self,
        own: &crate::share::audit::OwnNames,
    ) -> Result<crate::share::audit::Audit, Error> {
        Ok(crate::share::audit::audit_against(&self.store, own)?)
    }

    fn index(&self) -> Result<Index, Error> {
        Ok(Index::open(self.store.clone(), &self.trunk_ref()?)?.time_limit(self.time_limit))
    }

    /// Finding recorded runs reads only notes, never discovers a trunk.
    fn locator(&self) -> Result<Index<Runs>, Error> {
        Ok(Index::open_runs(self.store.clone())?)
    }
}

/// The questions, answered together at one pin.
pub struct At<'a> {
    db: &'a Connection,
    pin: Pin,
}

impl At<'_> {
    /// The pin every answer here is computed at: the trunk tip, and each
    /// kind's notes tip.
    #[must_use]
    pub const fn pin(&self) -> &Pin {
        &self.pin
    }

    /// Refuses a question about landings when the trunk ref names no commit.
    fn trunk(&self) -> Result<(), Error> {
        if self.pin.trunk.is_none() {
            return Err(StoreError::NoTrunk.into());
        }
        Ok(())
    }

    /// The trunk's newest `last` landings, each with the runs of each kind
    /// recorded on its tree.
    ///
    /// # Errors
    /// No trunk; an index failure.
    pub fn overview(&self, last: NonZeroU32) -> Result<Overview, Error> {
        self.trunk()?;
        ask::overview(self.db, last)
    }

    /// Every ended run's value of one key's metric over the selected
    /// landings, oldest first.
    ///
    /// # Errors
    /// No trunk; [`NotRecorded`] for a key or metric nothing records; an
    /// index failure.
    pub fn trend(&self, query: &TrendQuery) -> Result<Vec<Point>, Error> {
        self.trunk()?;
        ask::trend(self.db, query)
    }

    /// Every run on a tree, or on each selected landing's tree (oldest
    /// landing first), with its outcome counts.
    ///
    /// # Errors
    /// [`NotRecorded`] for a tree with no run; no trunk, for landings; an
    /// index failure.
    pub fn log(&self, query: &LogQuery) -> Result<Vec<LogEntry>, Error> {
        if matches!(query, LogQuery::Landings(_)) {
            self.trunk()?;
        }
        ask::log(self.db, query)
    }

    /// The runs `show` names (one run, or every run on a tree), each with
    /// the outcomes its globs keep.
    ///
    /// # Errors
    /// [`NotRecorded`] for an unknown run, a tree with no run, or a key or
    /// metric glob nothing matches; an index failure.
    pub fn show(&self, query: &ShowQuery) -> Result<Vec<RunDetail>, Error> {
        ask::show(self.db, query)
    }

    /// One run and every outcome it recorded.
    ///
    /// # Errors
    /// [`NotRecorded`] for an unknown or unreadable run; an index failure.
    pub fn run(&self, run: &RunId) -> Result<RunDetail, Error> {
        // `show` of one run id answers that run, or refuses it as not recorded.
        let [detail] = <[RunDetail; 1]>::try_from(ask::show(self.db, &ShowQuery::of(run))?)
            .expect("show of one run id answers exactly that run");
        Ok(detail)
    }

    /// The keys and metrics recorded, optionally of one kind and metrics
    /// matching a glob.
    ///
    /// # Errors
    /// [`NotRecorded`] for a glob nothing matches; an index failure.
    pub fn keys(&self, query: &KeysQuery) -> Result<Vec<KeyInfo>, Error> {
        ask::keys(self.db, query)
    }

    /// The numbers that differ between two trees, each beside its
    /// counterpart.
    ///
    /// # Errors
    /// [`NotRecorded`] for a tree with no run, an unknown key or a glob
    /// nothing matches; an index failure.
    pub fn diff(&self, query: &DiffQuery) -> Result<Vec<Difference>, Error> {
        ask::diff(self.db, query)
    }
}
