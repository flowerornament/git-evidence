//! The index handle: opening this build's file, catching it up, and reading
//! one snapshot of it under a time limit, with the pin from the same
//! snapshot. A notes-only writer has no trunk and cannot answer queries;
//! recording updates the notes' pin without changing cached landing history.

use super::catch_up::Reach;
use super::per_build::{file_name, holder_of, remove_unused, wal_of};
use super::schema::{VIEWS_VERSION, prepared};
use crate::format::id::{Kind, ObjectId, RunId, TreeId};
use crate::store::{NOTES, Store};
use crate::write::{RecordError, RunLocator};
use git_process::{Git, GitError};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How long a read may run before it is stopped, unless the caller sets
/// another limit: one read's snapshot pins the write-ahead log, so a
/// runaway query once grew a shared store's log to 6.9 GB and every writer
/// waited on it.
pub const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The write-ahead log's size past which a writer truncates it after
/// committing, when no reader holds it.
const WAL_TRUNCATE_BYTES: u64 = 64 << 20;

/// An index failure.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("index database: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A read ran past its time limit and was stopped; its snapshot is
    /// released.
    #[error(
        "the read ran past its time limit of {} s and was stopped; narrow it, or raise the limit (--time-limit)",
        limit.as_secs_f64()
    )]
    TimeLimit { limit: std::time::Duration },
    #[error("cannot start the read's time limit: {0}")]
    Watch(#[source] std::io::Error),
    #[error("cannot create {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The state an answer was computed at: the trunk tip, every kind's notes tip,
/// and the views version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Pin {
    pub trunk: Option<ObjectId>,
    /// Kind → notes tip.
    pub notes: BTreeMap<String, ObjectId>,
    pub views_version: u32,
}

/// The lock every open in an index directory takes while it sets up its
/// connection, whichever build's file it opens.
const OPENING: &str = "index-opening.lock";

/// The cache, opened at the shared store.
pub struct Index<Trunk = String> {
    store: Store,
    pub(super) db: Connection,
    /// The file's lock, held shared while `db` is open (declared after it,
    /// so it is released after the connection closes): a remover of
    /// another build's file proves no connection by taking it exclusively.
    _holder: std::fs::File,
    path: PathBuf,
    trunk_ref: Trunk,
    time_limit: std::time::Duration,
}

/// A notes-only index can locate runs, but cannot answer history queries.
pub struct Runs;

impl<Trunk> Index<Trunk> {
    /// Opens (creating if needed) this build's cache with its typed reading scope.
    /// Opens in one directory set up their connections in turn, and then
    /// wait for each other's writes with the bounded contention wait.
    ///
    /// # Errors
    /// Refuses when the database cannot be created or its schema is unusable.
    fn open_scoped(store: Store, trunk_ref: Trunk) -> Result<Self, IndexError> {
        let dir = store.state_dir();
        std::fs::create_dir_all(&dir).map_err(|source| IndexError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join(file_name());
        if !path.exists() {
            remove_unused(&dir, &path);
        }
        let io = |source| IndexError::Io {
            path: holder_of(&path),
            source,
        };
        let holder = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(holder_of(&path))
            .map_err(io)?;
        holder.lock_shared().map_err(io)?;
        // Switching a fresh file to WAL takes SQLite's exclusive lock and
        // fails at once, with no busy wait, while another opener reads the
        // file; opens in this directory therefore take their pragmas in
        // turn. This lock is held only for the open and is not the holder,
        // which an open Index keeps shared while a nested open runs.
        let opening_path = dir.join(OPENING);
        let opening = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&opening_path)
            .map_err(|source| IndexError::Io {
                path: opening_path.clone(),
                source,
            })?;
        opening.lock().map_err(|source| IndexError::Io {
            path: opening_path,
            source,
        })?;
        let db = Connection::open(&path)?;
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        // A crash or power loss can lose the last commits, never tear one
        // (WAL), and the index is a cache of the notes: a lost commit is
        // folded again. Commits then skip the fsync.
        db.pragma_update(None, "synchronous", "NORMAL")?;
        drop(opening);
        // Preparing waits for the first catch-up: it drops every table, and
        // only the fold in the same transaction fills them again.
        Ok(Self {
            store,
            db,
            _holder: holder,
            path,
            trunk_ref,
            time_limit: TIME_LIMIT,
        })
    }

    pub(super) fn git(&self) -> &Git {
        self.store.git()
    }
}

impl Index {
    /// Opens the shared cache for answers about this trunk.
    pub fn open(store: Store, trunk_ref: &str) -> Result<Self, IndexError> {
        Self::open_scoped(store, trunk_ref.to_owned())
    }

    /// Stops each read that runs longer than `limit`, instead of [`TIME_LIMIT`].
    #[must_use]
    pub const fn time_limit(mut self, limit: std::time::Duration) -> Self {
        self.time_limit = limit;
        self
    }

    /// The limit each read runs under.
    #[must_use]
    pub const fn read_limit(&self) -> std::time::Duration {
        self.time_limit
    }

    /// The index's file.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The ref this index reads landings from.
    #[must_use]
    pub fn trunk_ref(&self) -> &str {
        &self.trunk_ref
    }

    /// The database, for the verbs' reads.
    #[must_use]
    pub const fn db(&self) -> &Connection {
        &self.db
    }

    /// Catches up to the current tips and runs `read` on the result, in one
    /// transaction: no other catch-up can interleave, so the pin returned is
    /// the state `read` saw.
    ///
    /// # Errors
    /// Refuses when Git or the database fails; the index is then left as it was.
    pub fn answer<T>(
        &mut self,
        read: impl FnOnce(&Self, Pin) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let trunk = self.trunk_ref.clone();
        self.locked(Reach::Answers(&trunk), |index, pin| {
            within(&index.db, index.time_limit, || read(index, pin))
        })
    }
}

impl<Trunk> Index<Trunk> {
    /// Catches up as far as `reach` and runs `then` in the same write
    /// transaction; commits, then shrinks the write-ahead log if it grew.
    fn locked<T>(
        &mut self,
        reach: Reach<'_>,
        then: impl FnOnce(&Self, Pin) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.catch_up_locked(reach).and_then(|pin| then(self, pin));
        match result {
            Ok(value) => {
                self.db.execute_batch("COMMIT")?;
                self.truncate_wal();
                Ok(value)
            }
            Err(error) => {
                // The failure being reported matters more than a failed rollback,
                // which SQLite completes when the connection closes anyway.
                self.db.execute_batch("ROLLBACK").ok();
                Err(error)
            }
        }
    }

    /// Truncates the write-ahead log once it passes [`WAL_TRUNCATE_BYTES`]
    /// (a refold writes the whole file into it), without waiting: a
    /// checkpoint that must wait for a reader is skipped, and the next
    /// commit tries again. The commit is durable either way, so this
    /// reports nothing.
    fn truncate_wal(&self) {
        let grown =
            std::fs::metadata(wal_of(&self.path)).is_ok_and(|wal| wal.len() > WAL_TRUNCATE_BYTES);
        if !grown {
            return;
        }
        // TRUNCATE waits on readers by calling the busy handler, and holds
        // off writers meanwhile: with no wait, it either finishes now or
        // reports busy and changes nothing.
        let waited = self.db.busy_timeout(std::time::Duration::ZERO).is_ok();
        self.db
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .ok();
        if waited {
            self.db
                .busy_timeout(std::time::Duration::from_secs(30))
                .ok();
        }
    }
}

impl Index {
    /// Catches up and returns the pin, for callers that read later.
    ///
    /// # Errors
    /// As [`Index::answer`].
    pub fn catch_up(&mut self) -> Result<Pin, IndexError> {
        let trunk = self.trunk_ref.clone();
        self.locked(Reach::Answers(&trunk), |_, pin| Ok(pin))
    }

    /// Like [`Index::answer`], but `read` runs in a read snapshot, not under the
    /// write lock: the index catches up in its own short transaction, then
    /// every question sees the state cached at that point, with the pin read
    /// from the same snapshot. Other processes are never held up by
    /// `read`. If another reader re-cached a different trunk, or a binary built
    /// from other source rebuilt the file, in between, the question falls
    /// back to [`Index::answer`].
    ///
    /// # Errors
    /// As [`Index::answer`].
    pub fn read<T>(
        &mut self,
        read: impl FnOnce(&Self, Pin) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        self.catch_up()?;
        self.read_caught_up(read)
    }

    /// [`Index::read`] after its catch-up: the snapshot, or the fallback.
    fn read_caught_up<T>(
        &mut self,
        read: impl FnOnce(&Self, Pin) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        self.db.execute_batch("BEGIN DEFERRED")?;
        let pin = match self.cached_pin() {
            Ok(Cached::Pin(pin)) => pin,
            Ok(Cached::OtherTrunk | Cached::OtherSource) => {
                self.db.execute_batch("ROLLBACK").ok();
                return self.answer(read);
            }
            Err(error) => {
                self.db.execute_batch("ROLLBACK").ok();
                return Err(error);
            }
        };
        let result = within(&self.db, self.time_limit, || read(self, pin));
        // Ending the read releases its snapshot, the limit's whole point.
        self.db.execute_batch("COMMIT").ok();
        result
    }

    /// The pin the cache holds, if it is for this index's trunk and build.
    fn cached_pin(&self) -> Result<Cached, IndexError> {
        pin_in(&self.db, &self.trunk_ref)
    }
}

impl Index<Runs> {
    /// Opens the shared cache for locating runs, without a trunk.
    pub fn open_runs(store: Store) -> Result<Self, IndexError> {
        Self::open_scoped(store, Runs)
    }

    /// Folds notes only; landing history and its pin remain untouched.
    pub fn catch_up_runs(&mut self) -> Result<(), IndexError> {
        self.locked(Reach::Runs, |_, _| Ok(()))
    }
}

impl<Trunk> Index<Trunk> {
    /// The tips this operation needs: writers never resolve a trunk.
    pub(super) fn current_tips(&self, reach: Reach<'_>) -> Result<Pin, IndexError> {
        let trunk = match reach {
            Reach::Answers(trunk) => self.git().resolve(trunk)?,
            Reach::Runs => None,
        };
        let listing = self.git().run(
            &["for-each-ref", "--format=%(refname) %(objectname)", NOTES],
            None,
        )?;
        let mut notes = BTreeMap::new();
        for line in String::from_utf8_lossy(&listing).lines() {
            if let Some((name, tip)) = line.split_once(' ')
                && let Some(kind) = name.strip_prefix(NOTES)
                && Kind::parse(kind).is_ok()
                && let Ok(tip) = ObjectId::parse(tip)
            {
                notes.insert(kind.to_owned(), tip);
            }
        }
        Ok(Pin {
            trunk,
            notes,
            views_version: VIEWS_VERSION,
        })
    }

    pub(super) fn cached_tips(&self) -> Result<BTreeMap<String, Option<String>>, IndexError> {
        cached_tips(&self.db)
    }
}

impl<Trunk> RunLocator for Index<Trunk> {
    fn tree_of_run(&self, kind: &Kind, run: &RunId) -> Result<Option<TreeId>, RecordError> {
        let tree: Option<String> = self
            .db
            .query_row(
                "SELECT tree FROM all_runs WHERE run = ?1 AND kind = ?2",
                params![run.as_str(), kind.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(RecordError::Locate)?;
        Ok(tree.and_then(|tree| TreeId::parse(&tree).ok()))
    }
}

fn cached_tips(db: &Connection) -> Result<BTreeMap<String, Option<String>>, IndexError> {
    let mut statement = db.prepare("SELECT ref, tip FROM pin")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What one snapshot of the index holds for a trunk.
pub(crate) enum Cached {
    /// The pin this build's index answers at.
    Pin(Pin),
    /// Another reader last cached another trunk.
    OtherTrunk,
    /// A binary built from other source has rebuilt the file since: nothing
    /// in it is this build's.
    OtherSource,
}

/// What `db` holds for `trunk_ref`, read in the caller's transaction.
pub(crate) fn pin_in(db: &Connection, trunk_ref: &str) -> Result<Cached, IndexError> {
    if !prepared(db)? {
        return Ok(Cached::OtherSource);
    }
    let cached = cached_tips(db)?;
    let Some(trunk) = cached.get(&format!("trunk:{trunk_ref}")) else {
        return Ok(Cached::OtherTrunk);
    };
    let trunk = match trunk {
        Some(tip) => Some(ObjectId::parse(tip).map_err(GitError::from)?),
        None => None,
    };
    let mut notes = BTreeMap::new();
    for (name, tip) in &cached {
        if let Some(kind) = name.strip_prefix("notes:")
            && let Some(tip) = tip
        {
            notes.insert(
                kind.to_owned(),
                ObjectId::parse(tip).map_err(GitError::from)?,
            );
        }
    }
    Ok(Cached::Pin(Pin {
        trunk,
        notes,
        views_version: VIEWS_VERSION,
    }))
}

/// Runs `read` on `db` under `limit`: past it, the statement running is
/// interrupted, and the read is refused as [`IndexError::TimeLimit`]
/// whatever it returned. The caller ends the transaction, which releases
/// the snapshot.
pub(crate) fn within<T, E: From<IndexError>>(
    db: &Connection,
    limit: std::time::Duration,
    read: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let interrupt = db.get_interrupt_handle();
    let (done, wait) = std::sync::mpsc::channel::<()>();
    let watch = std::thread::Builder::new()
        .name("git-evidence-time-limit".into())
        .spawn(move || match wait.recv_timeout(limit) {
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                interrupt.interrupt();
                true
            }
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => false,
        })
        .map_err(IndexError::Watch)?;
    let result = read();
    drop(done);
    // A watcher that panicked enforced nothing; the read's own result stands.
    if watch.join().unwrap_or(false) {
        return Err(IndexError::TimeLimit { limit }.into());
    }
    result
}

#[cfg(test)]
mod tests {
    //! Another build's preparation, written between this build's open and
    //! its answers, never reaches an answer; a read past its limit is
    //! stopped; a writer's catch-up pins the notes it folded.

    use super::{Cached, Index, IndexError, pin_in};
    use crate::index::fixtures::{TRUNK, another_build_prepares, index, record_a_run};
    use crate::testing::{HANG_GUARD, Scratch};
    use rusqlite::Connection;
    use std::time::Duration;

    /// A query that runs for hours: counting to ten billion, one row at a time.
    const SLOW: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c \
                        WHERE x < 10000000000) SELECT max(x) FROM c";

    /// `measures`' columns and the folded `violations` rows, as `db` sees them.
    fn prepared_as(db: &Connection) -> Result<(Vec<String>, i64), IndexError> {
        let columns = db
            .prepare("SELECT name FROM pragma_table_info('measures') ORDER BY cid")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        let rows = db.query_row("SELECT count(*) FROM violations", [], |row| row.get(0))?;
        Ok((columns, rows))
    }

    fn this_builds(prepared: &(Vec<String>, i64)) {
        assert!(
            prepared.0.iter().any(|c| c == "metric"),
            "views recreated: {:?}",
            prepared.0
        );
        assert_eq!(prepared.1, 0, "violations refolded by this build");
    }

    /// Drivers sharing one store open its fresh index at the same moment and
    /// catch up: each either opens and answers or waits, never refuses with
    /// "database is locked".
    #[test]
    fn drivers_opening_one_fresh_index_together_each_open() {
        use super::Index;
        use crate::store::Store;
        const DRIVERS: usize = 4;
        let scratch = Scratch::new().expect("scratch");
        let git = scratch.path().join(".git");
        let state = Store::open(git.clone()).state_dir();
        for round in 0..24 {
            assert!(!state.exists(), "round {round} starts without an index");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(DRIVERS));
            let outcomes: Vec<_> = (0..DRIVERS)
                .map(|_| {
                    let git = git.clone();
                    let barrier = std::sync::Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        Index::open(Store::open(git), TRUNK)
                            .and_then(|mut index| index.catch_up().map(|_| ()))
                            .map_err(|error| error.to_string())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|driver| driver.join().expect("a driver completes"))
                .collect();
            assert!(
                outcomes.iter().all(Result::is_ok),
                "round {round}: {outcomes:?}"
            );
            // Every driver has joined and dropped its index. Keep the unchanged
            // Git source, but race a fresh index and fresh locks next round.
            std::fs::remove_dir_all(&state).expect("remove private index state");
        }
    }

    /// An open index holds its file shared; a second open of the same
    /// store while it stays open (a nested `Evidence::at`) opens and
    /// answers, never waiting on the first.
    #[test]
    fn an_index_opens_while_another_of_its_store_stays_open() {
        use super::Index;
        use crate::store::Store;
        let scratch = Scratch::new().expect("scratch");
        let git = scratch.path().join(".git");
        let mut outer = Index::open(Store::open(git.clone()), TRUNK).expect("the outer opens");
        outer.catch_up().expect("the outer catches up");
        let (done, completed) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let inner = Index::open(Store::open(git), TRUNK)
                .and_then(|mut index| index.catch_up().map(|_| ()))
                .map_err(|error| error.to_string());
            done.send(inner).ok();
        });
        let inner = completed
            .recv_timeout(HANG_GUARD)
            .expect("the nested open does not wait on the open index");
        assert!(inner.is_ok(), "{inner:?}");
        outer.catch_up().expect("the outer still answers");
    }

    #[test]
    fn opening_waits_for_a_writer_before_the_first_locking_pragma() {
        use super::Index;
        use crate::index::per_build::file_name;
        use crate::store::Store;
        use std::sync::mpsc;

        let scratch = Scratch::new().expect("scratch");
        let store = Store::open(scratch.path().join(".git"));
        let directory = store.state_dir();
        std::fs::create_dir_all(&directory).expect("private index directory");
        let writer = Connection::open(directory.join(file_name())).expect("blocking writer");
        let default_ms: i64 = writer
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .expect("SQLite default wait");
        let hold =
            Duration::from_millis(u64::try_from(default_ms).expect("nonnegative default wait"))
                + Duration::from_secs(1);
        assert!(
            hold < Duration::from_secs(30),
            "exercise the gap before the declared index wait"
        );
        writer
            .execute_batch("BEGIN EXCLUSIVE; CREATE TABLE held(value);")
            .expect("hold first initialization lock");
        let (started, entered) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let opener = std::thread::spawn(move || {
            started.send(()).expect("announce opener");
            done.send(Index::open(store, TRUNK).map(|_| ()))
                .expect("report open");
        });
        entered.recv().expect("opener starts with writer held");
        let before_release = completed.recv_timeout(hold);
        // Release and join before assertions, including when the old setup fails.
        writer.execute_batch("COMMIT").expect("release writer");
        let opened = match &before_release {
            Ok(_) => None,
            Err(_) => Some(
                completed
                    .recv_timeout(HANG_GUARD)
                    .expect("open after grant"),
            ),
        };
        opener.join().expect("retire opener");
        assert!(
            matches!(before_release, Err(mpsc::RecvTimeoutError::Timeout)),
            "first pragma must use the index contention policy: {before_release:?}"
        );
        opened
            .expect("waited for grant")
            .expect("initialize after writer retires");
    }

    #[test]
    fn a_catch_up_re_prepares_what_another_build_prepared_since_open() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let mut index = index(&scratch);
        another_build_prepares(index.path());
        index.catch_up().expect("catch up");
        this_builds(&prepared_as(index.db()).expect("read"));
    }

    #[test]
    fn a_snapshot_another_build_prepared_holds_no_pin_of_this_build() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let index = index(&scratch);
        assert!(matches!(
            pin_in(index.db(), TRUNK).expect("pin"),
            Cached::Pin(_)
        ));
        another_build_prepares(index.path());
        assert!(matches!(
            pin_in(index.db(), TRUNK).expect("pin"),
            Cached::OtherSource
        ));
    }

    #[test]
    fn a_read_after_another_build_prepared_falls_back_to_this_builds_answer() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let mut index = index(&scratch);
        another_build_prepares(index.path());
        let prepared = index
            .read_caught_up(|ix, _| prepared_as(ix.db()))
            .expect("this build's answer");
        this_builds(&prepared);
    }

    /// A read past its limit is stopped with a typed refusal, and its
    /// snapshot is gone: a truncating checkpoint, which no reader may
    /// hold up, then completes. Without the limit
    /// the query runs for hours, and the test fails at its own deadline.
    #[test]
    fn a_read_past_its_time_limit_is_refused_and_holds_no_snapshot() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let mut index = index(&scratch).time_limit(Duration::from_millis(200));
        let (done, finished) = std::sync::mpsc::channel();
        let path = index.path().to_owned();
        std::thread::spawn(move || {
            let refused =
                index.read(|ix, _| Ok(ix.db().query_row(SLOW, [], |row| row.get::<_, i64>(0))?));
            done.send(refused).ok();
            // The connection stays open: what is released is the snapshot.
            std::thread::park();
        });
        let refused = finished
            .recv_timeout(HANG_GUARD)
            .expect("the read was stopped");
        assert!(
            matches!(refused, Err(IndexError::TimeLimit { limit }) if limit == Duration::from_millis(200)),
            "{refused:?}"
        );
        let busy: i64 = Connection::open(path)
            .expect("second connection")
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .expect("checkpoint");
        assert_eq!(busy, 0, "no reader holds the log");
    }

    /// A writer folds only the notes, and its pin says so: a snapshot after
    /// it reads the new run at a pin that names its notes.
    #[test]
    fn a_writers_catch_up_folds_the_notes_and_pins_them() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let index = index(&scratch);
        record_a_run(&scratch);
        Index::open_runs(crate::store::Store::open(scratch.path().join(".git")))
            .expect("notes index")
            .catch_up_runs()
            .expect("writer's catch-up");
        let Cached::Pin(pin) = pin_in(index.db(), TRUNK).expect("pin") else {
            panic!("a writer's catch-up leaves a pin");
        };
        assert!(pin.notes.contains_key("perf"), "{pin:?}");
        let runs: i64 = index
            .db()
            .query_row("SELECT count(*) FROM runs", [], |row| row.get(0))
            .expect("runs");
        assert_eq!(runs, 1);
    }

    #[test]
    fn a_notes_writer_preserves_an_alternate_trunks_pin_and_landings() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(2)
            .expect("landings");
        let store = crate::store::Store::open(scratch.path().join(".git"));
        store
            .git()
            .update_ref("refs/heads/alternate", scratch.landing(0).object(), None)
            .expect("alternate trunk");
        let mut reader = Index::open(store.clone(), "refs/heads/alternate").expect("reader");
        let before = reader.catch_up().expect("first answer");
        let pins = reader.cached_tips().expect("cached pins");
        reader
            .db
            .execute_batch(
                "CREATE TRIGGER no_repeated_landings BEFORE DELETE ON trunk
             BEGIN SELECT RAISE(ABORT, 'unchanged landings must not refold'); END;",
            )
            .expect("independent refold guard");
        record_a_run(&scratch);
        let mut writer = Index::open_runs(store).expect("notes index");
        writer.catch_up_runs().expect("notes only");
        let after = writer.cached_tips().expect("writer pins");
        for (key, value) in pins.iter().filter(|(key, _)| key.starts_with("trunk:")) {
            assert_eq!(after.get(key), Some(value), "preserve {key}");
        }
        let warm = reader
            .catch_up()
            .expect("unchanged trunk is not folded again");
        assert_eq!(warm.trunk, before.trunk);
        assert!(warm.notes.contains_key("perf"));
        assert_eq!(
            reader
                .db
                .query_row("SELECT count(*) FROM trunk", [], |r| r.get::<_, i64>(0))
                .expect("landings"),
            1
        );
    }

    #[test]
    fn a_cold_notes_writer_leaves_landings_for_the_first_answer() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(2)
            .expect("landings");
        record_a_run(&scratch);
        let store = crate::store::Store::open(scratch.path().join(".git"));
        let mut writer = Index::open_runs(store.clone()).expect("notes index");
        writer.catch_up_runs().expect("first notes fold");
        assert!(
            writer
                .cached_tips()
                .expect("pins")
                .keys()
                .all(|key| !key.starts_with("trunk:"))
        );
        assert_eq!(
            writer
                .db
                .query_row("SELECT count(*) FROM trunk", [], |r| r.get::<_, i64>(0))
                .expect("no landings"),
            0
        );
        assert_eq!(
            writer
                .db
                .query_row("SELECT count(*) FROM runs", [], |r| r.get::<_, i64>(0))
                .expect("runs"),
            1
        );
        let mut reader = Index::open(store, TRUNK).expect("reader");
        let cold = reader.catch_up().expect("first answer folds landings");
        let warm = reader.catch_up().expect("warm answer");
        assert_eq!(cold, warm);
        assert_eq!(
            reader
                .db
                .query_row("SELECT count(*) FROM trunk", [], |r| r.get::<_, i64>(0))
                .expect("landings"),
            2
        );
    }
}
