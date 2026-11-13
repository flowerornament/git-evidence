//! The index's files: one per build source, named for it ([`file_name`]), so
//! binaries built from other source sharing a store each keep their own. A
//! new file removes the other builds' files no one has used for a day, and
//! never one a connection may still hold open.

use super::schema::VIEWS_VERSION;
use std::path::PathBuf;

/// The source this binary is built from: a digest of the crate's `src/`
/// (`build.rs`). The index records the source that made it; a binary built
/// from any other rebuilds it from the notes, so no answer reads another
/// build's folding, rules or definitions.
pub(super) const SOURCE: &str = env!("GIT_EVIDENCE_SOURCE");

/// The index's file in the store's state directory. Each build (views
/// version and source) has its own file, so binaries built from different
/// source sharing a store (workspaces at different commits, a CI runner's
/// older tool) each keep their own cache instead of dropping and rebuilding
/// one file under each other's write lock.
#[must_use]
pub fn file_name() -> String {
    let source = SOURCE.strip_prefix("sha256:").unwrap_or(SOURCE);
    let short = source.get(..12).unwrap_or(source);
    format!("index-v{VIEWS_VERSION}-{short}.sqlite")
}

/// How long another build's index file may go unused before the next new
/// file removes it: a day. Every answer writes its file (the pin), so a file
/// this old belongs to a binary no one has run since; it is a cache, and a
/// cold rebuild remakes it.
pub(super) const UNUSED: std::time::Duration = std::time::Duration::from_hours(24);

/// How many other builds' index files a store keeps once they are idle for
/// [`IDLE`]: each is a whole fold (hundreds of MB on a large store), so disk
/// use stays bounded however many builds share it.
const KEPT: usize = 3;

/// How long another build's file must be idle before the cap ([`KEPT`]) may
/// remove it: far longer than any catch-up and read, so a file between two
/// questions of one session is not evicted to be rebuilt cold.
const IDLE: std::time::Duration = std::time::Duration::from_hours(1);

/// The lock file beside the index file at `db`. Every [`super::Index`] holds it
/// shared while its connections are open; a remover that takes it
/// exclusively without waiting has proved that no connection of this
/// scheme is open, nor can one open until it is released. It is never
/// removed: a waiting opener holds its inode.
pub(super) fn holder_of(db: &std::path::Path) -> PathBuf {
    db.with_extension("sqlite.lock")
}

/// Another build's index file, and how it may be removed.
struct Other {
    db: PathBuf,
    /// The newest modification of the file, its log or its shared memory.
    used: std::time::SystemTime,
}

impl Other {
    fn companions(&self) -> [PathBuf; 2] {
        [wal_of(&self.db), self.db.with_extension("sqlite-shm")]
    }

    /// Removes the file if no connection can have it open. A file of this
    /// scheme is proved closed by its exclusive lock, held while every
    /// file goes: its log and shared memory (left by a crash) first, then
    /// the database. A file an older binary made has no lock: it goes only
    /// when it has no shared memory or log, which every open WAL connection
    /// has, and only the database is removed, so no log is left to pair
    /// with a database made later under its name.
    fn remove(&self) {
        match std::fs::File::open(holder_of(&self.db)) {
            Ok(holder) => {
                if holder.try_lock().is_ok() {
                    for file in self.companions() {
                        std::fs::remove_file(file).ok();
                    }
                    std::fs::remove_file(&self.db).ok();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.companions().iter().all(|file| !file.exists()) {
                    std::fs::remove_file(&self.db).ok();
                }
            }
            Err(_) => {}
        }
    }

    /// Whether the file has its lock: one this scheme opens.
    fn locked_scheme(&self) -> bool {
        holder_of(&self.db).exists()
    }
}

/// Removes other builds' index files in `dir` (never `ours`) that no one
/// can have open ([`Other::remove`]): those idle for [`UNUSED`], and, of
/// those idle for [`IDLE`] and opened under the lock, all but the [`KEPT`]
/// most recently used. Best effort: a file that cannot be read or removed
/// stays, and costs only its disk.
pub(super) fn remove_unused(dir: &std::path::Path, ours: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut others: Vec<Other> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path != ours
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name.starts_with("index") && name.ends_with(".sqlite"))
        })
        .map(|db| {
            let used = [db.clone(), wal_of(&db), db.with_extension("sqlite-shm")]
                .iter()
                .filter_map(|file| std::fs::metadata(file).and_then(|m| m.modified()).ok())
                .max()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Other { db, used }
        })
        .collect();
    others.sort_by_key(|other| std::cmp::Reverse(other.used));
    let now = std::time::SystemTime::now();
    let idle = |other: &Other, floor| now.duration_since(other.used).is_ok_and(|age| age > floor);
    for (rank, other) in others.iter().enumerate() {
        let capped = rank >= KEPT && idle(other, IDLE) && other.locked_scheme();
        if idle(other, UNUSED) || capped {
            other.remove();
        }
    }
}

/// The write-ahead log beside the index file at `path`.
pub(super) fn wal_of(path: &std::path::Path) -> PathBuf {
    path.with_extension("sqlite-wal")
}

#[cfg(test)]
mod tests {
    use super::{UNUSED, remove_unused};
    use rusqlite::Connection;
    use std::time::Duration;

    /// Another build's file goes only when no connection can have it
    /// open: idle a day, or idle an hour and past the three most recently
    /// used; and proved closed, by its lock or (made by an older binary,
    /// without one) by having no log or shared memory.
    #[test]
    fn another_builds_file_is_removed_only_when_no_connection_can_have_it_open() {
        let dir = tempfile::tempdir().expect("dir");
        let ago = |seconds: u64| std::time::SystemTime::now() - Duration::from_secs(seconds);
        let day = UNUSED.as_secs() + 60;
        let touch = |name: &str, modified: std::time::SystemTime| {
            let path = dir.path().join(name);
            let handle = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .expect("file");
            handle.set_modified(modified).expect("mtime");
            path
        };
        // This scheme's file: the database and its lock.
        let locked = |name: &str, modified| {
            touch(&format!("{name}.sqlite.lock"), modified);
            touch(&format!("{name}.sqlite"), modified)
        };
        let old_unlocked = touch("index-v23.sqlite", ago(day));
        let old_with_log = touch("index-v22.sqlite", ago(day));
        touch("index-v22.sqlite-wal", ago(day));
        // An older binary's connection, open: its shared memory exists.
        let open_unlocked = dir.path().join("index-v24.sqlite");
        let connection = Connection::open(&open_unlocked).expect("open");
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .expect("WAL");
        for file in [
            open_unlocked.clone(),
            open_unlocked.with_extension("sqlite-wal"),
            open_unlocked.with_extension("sqlite-shm"),
        ] {
            touch(
                file.file_name().and_then(|n| n.to_str()).expect("name"),
                ago(day),
            );
        }
        // This scheme's file held by an open index: its lock, shared.
        let held = locked("index-v25-aaaaaaaaaaaa", ago(day));
        let holder = std::fs::File::open(held.with_extension("sqlite.lock")).expect("lock");
        holder.lock_shared().expect("shared");
        let old_locked = locked("index-v25-bbbbbbbbbbbb", ago(day));
        // Recently used past the cap: kept; idle an hour past the cap: gone.
        let recent: Vec<_> = (1..=4)
            .map(|n| locked(&format!("index-v25-00000000000{n}"), ago(60 * n)))
            .collect();
        let idle_past_cap = locked("index-v25-000000000009", ago(2 * 60 * 60));
        let other = touch("notes.lock", ago(day));
        let ours = dir.path().join("index-v25-cccccccccccc.sqlite");
        remove_unused(dir.path(), &ours);
        assert!(!old_unlocked.exists(), "idle a day, closed: removed");
        assert!(!old_locked.exists(), "idle a day, its lock taken: removed");
        assert!(
            !idle_past_cap.exists(),
            "idle an hour, past the cap: removed"
        );
        assert!(old_with_log.exists(), "a log: maybe open, kept");
        assert!(open_unlocked.exists(), "an open connection's file: kept");
        assert!(held.exists(), "held by an open index: kept");
        assert!(
            recent.iter().all(|f| f.exists()),
            "used within the hour: kept"
        );
        assert!(other.exists(), "not an index file: kept");
        drop(connection);
    }
}
