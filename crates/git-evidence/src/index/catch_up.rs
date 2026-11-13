//! Catching up: what the index is behind on since its pin (the trunk and the
//! notes), and folding only that: the landings (an answer only) and the notes
//! whose blobs changed.

use super::handle::{Index, IndexError, Pin};
use super::note_rows::{delete_note_rows, insert_note};
use super::schema::{LOOKUPS, RUN_ROWS, execute_cached, prepare, prepared, sql_integer};
use crate::format::id::{ObjectId, TreeId};
use git_process::{Git, GitError};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

/// One landing on the trunk's first-parent line.
struct TrunkCommit {
    commit: String,
    tree: String,
    /// Committer time, Unix seconds.
    committed: i64,
    /// The commit message's first line.
    subject: String,
}

/// What a catch-up finds: the tips now, and the tips the index last folded.
struct Lag {
    now: Pin,
    /// The `pin` table: `trunk:<ref>` and `notes:<kind>` → tip; a trunk
    /// that did not resolve has no tip.
    folded: BTreeMap<String, Option<String>>,
}

impl Lag {
    /// The trunk's tip as the pin spells it. An unresolved trunk is cached
    /// too, with no tip, so a warm cache never answers for it with another
    /// trunk's landings.
    fn trunk_now(&self) -> Option<String> {
        self.now.trunk.as_ref().map(ToString::to_string)
    }

    fn trunk_moved(&self, trunk: &str) -> bool {
        self.folded.get(&format!("trunk:{trunk}")) != Some(&self.trunk_now())
    }

    fn a_kind_vanished(&self) -> bool {
        self.folded.keys().any(|name| {
            name.strip_prefix("notes:")
                .is_some_and(|kind| !self.now.notes.contains_key(kind))
        })
    }

    /// Records the pin folded as far as `reach`: a writer leaves the trunk
    /// at the tip the landings were last folded from.
    fn record(&self, db: &Connection, reach: Reach<'_>) -> rusqlite::Result<()> {
        match reach {
            Reach::Answers(trunk) => {
                execute_cached(db, "DELETE FROM pin", [])?;
                execute_cached(
                    db,
                    "INSERT INTO pin (ref, tip) VALUES (?1, ?2)",
                    params![format!("trunk:{trunk}"), self.trunk_now()],
                )?;
            }
            Reach::Runs => {
                execute_cached(db, "DELETE FROM pin WHERE ref LIKE 'notes:%'", [])?;
            }
        }
        let notes = self
            .now
            .notes
            .iter()
            .map(|(kind, tip)| (format!("notes:{kind}"), Some(tip.to_string())));
        for (name, tip) in notes {
            execute_cached(
                db,
                "INSERT INTO pin (ref, tip) VALUES (?1, ?2)",
                params![name, tip],
            )?;
        }
        Ok(())
    }
}

/// The notes a catch-up folds: per kind, each changed note, and the bytes
/// of every blob it reads.
#[derive(Default)]
struct NotesBehind {
    changes: Vec<(String, Vec<NoteChange>)>,
    blobs: BTreeMap<ObjectId, Vec<u8>>,
    /// Every note is folded afresh, after the folded rows are cleared.
    refold: bool,
}

impl NotesBehind {
    fn afters(&self) -> impl Iterator<Item = &ObjectId> {
        self.changes
            .iter()
            .flat_map(|(_, changed)| changed.iter().filter_map(|note| note.after.as_ref()))
    }

    /// Whether a changed note lost a line it held, or was removed.
    fn any_lost_a_line(&self) -> bool {
        self.changes
            .iter()
            .flat_map(|(_, c)| c)
            .any(|note| match (&note.before, &note.after) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(before), Some(after)) => {
                    let kept: BTreeSet<&[u8]> = self.blobs[after].split(|b| *b == b'\n').collect();
                    self.blobs[before]
                        .split(|b| *b == b'\n')
                        .any(|line| !kept.contains(line))
                }
            })
    }

    /// Folds every changed note into the tables (after clearing them for a
    /// refold).
    fn fold(self, db: &Connection) -> rusqlite::Result<()> {
        if self.refold {
            for table in RUN_ROWS
                .iter()
                .chain(&["all_runs", "violations", "sightings"])
            {
                execute_cached(db, &format!("DELETE FROM {table}"), [])?;
            }
        }
        for (kind, notes) in &self.changes {
            for note in notes {
                // A note that was absent left no rows to delete: the whole
                // cold fold, a refold, and every new tree after it.
                if note.before.is_some() && !self.refold {
                    delete_note_rows(db, kind, &note.tree)?;
                }
                if let Some(blob) = &note.after {
                    insert_note(db, kind, &note.tree, &self.blobs[blob])?;
                }
            }
        }
        Ok(())
    }
}

/// A notes commit's entries: each note's blob, keyed by the annotated
/// object's id (its path with the fanout's slashes removed).
type NoteEntries = BTreeMap<String, ObjectId>;

/// One note that differs between two tips of a kind's ref.
pub(super) struct NoteChange {
    tree: TreeId,
    before: Option<ObjectId>,
    after: Option<ObjectId>,
}

/// Populate this catch-up's object table atomically, without rereading objects.
fn read_missing_blobs<'a>(
    git: &Git,
    ids: impl Iterator<Item = &'a ObjectId>,
    blobs: &mut BTreeMap<ObjectId, Vec<u8>>,
) -> Result<(), GitError> {
    let missing: Vec<_> = ids
        .filter(|id| !blobs.contains_key(*id))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let bodies = git.read_blobs(&missing)?;
    blobs.extend(missing.into_iter().zip(bodies));
    Ok(())
}

/// How far a catch-up brings the index.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach<'a> {
    /// Every table: an answer reads it.
    Answers(&'a str),
    /// The tables folded from the notes, enough to find where a run is
    /// recorded; the landings wait for the next [`Reach::Answers`] catch-up.
    /// A writer needs no more.
    Runs,
}

impl<Trunk> Index<Trunk> {
    /// Brings the index as far as `reach`, in the caller's write transaction:
    /// prepares a file another build made, finds what it is behind on, folds
    /// the landings (an answer only) and the changed notes, and records the
    /// pin folded.
    pub(super) fn catch_up_locked(&self, reach: Reach<'_>) -> Result<Pin, IndexError> {
        let db = &self.db;
        // A new file, or one another build made (before `open` or since):
        // dropped and refolded here, in this one write transaction, so every
        // committed state of the file is one build's complete fold.
        if !prepared(db)? {
            prepare(db)?;
        }
        let lag = Lag {
            now: self.current_tips(reach)?,
            folded: self.cached_tips()?,
        };
        if let Reach::Answers(trunk) = reach
            && lag.trunk_moved(trunk)
        {
            self.fold_landings(lag.now.trunk.as_ref())?;
        }
        self.notes_behind(&lag)?.fold(db)?;
        db.execute_batch(LOOKUPS)?;
        lag.record(db, reach)?;
        crate::write::fault::point("index:folded");
        Ok(lag.now)
    }

    /// Refolds the landings from the trunk at `tip` (`None`: it does not
    /// resolve): the first-parent line.
    fn fold_landings(&self, tip: Option<&ObjectId>) -> Result<(), IndexError> {
        let db = &self.db;
        let landings = match tip {
            Some(tip) => self.first_parent_landings(tip)?,
            None => Vec::new(),
        };
        execute_cached(db, "DELETE FROM trunk", [])?;
        for (number, landing) in landings.iter().enumerate() {
            execute_cached(
                db,
                "INSERT INTO trunk (landing, commit_id, tree, committed, subject)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    sql_integer(number)?,
                    landing.commit,
                    landing.tree,
                    landing.committed,
                    landing.subject
                ],
            )?;
        }
        Ok(())
    }

    /// The notes the index is behind on, with their bytes. Notes only grow
    /// in normal use, and only what changed is folded. When a kind's ref
    /// vanished, or a note lost a line (a rewound ref, a removed or edited
    /// note), runs it excluded elsewhere may be readable again: every note
    /// is folded afresh, so the answer is a rebuild's.
    fn notes_behind(&self, lag: &Lag) -> Result<NotesBehind, IndexError> {
        let mut behind = NotesBehind::default();
        for (kind, tip) in &lag.now.notes {
            let old = lag
                .folded
                .get(&format!("notes:{kind}"))
                .and_then(Option::as_deref);
            if old != Some(tip.as_str()) {
                let changed = self.changed_notes(old, tip)?;
                behind.changes.push((kind.clone(), changed));
            }
        }
        behind.refold = lag.a_kind_vanished();
        // These exact objects belong only to this catch-up. Read changed note
        // bytes together and use the same after bytes for detection and folding.
        let befores = behind.changes.iter().flat_map(|(_, changed)| {
            changed
                .iter()
                .flat_map(|note| note.before.iter().filter(|_| !behind.refold))
        });
        let afters = behind
            .changes
            .iter()
            .flat_map(|(_, changed)| changed.iter().flat_map(|note| note.after.iter()));
        read_missing_blobs(self.git(), befores.chain(afters), &mut behind.blobs)?;
        behind.refold = behind.refold || behind.any_lost_a_line();
        if behind.refold {
            behind.changes = lag
                .now
                .notes
                .iter()
                .map(|(kind, tip)| Ok((kind.clone(), self.changed_notes(None, tip)?)))
                .collect::<Result<_, IndexError>>()?;
            let needed: BTreeSet<ObjectId> = behind.afters().cloned().collect();
            behind.blobs.retain(|id, _| needed.contains(id));
            // A refold can need notes that did not change, so that catching
            // up equals a rebuild: batch only the objects the rebuild adds.
            read_missing_blobs(self.git(), needed.iter(), &mut behind.blobs)?;
        }
        Ok(behind)
    }

    /// Landings, oldest first: the trunk's first-parent line.
    fn first_parent_landings(&self, tip: &ObjectId) -> Result<Vec<TrunkCommit>, IndexError> {
        let out = self.git().run(
            &[
                "log",
                "--first-parent",
                "--reverse",
                "--format=%H%x09%T%x09%ct%x09%s",
                tip.as_str(),
            ],
            None,
        )?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                Some(TrunkCommit {
                    commit: fields.next()?.to_owned(),
                    tree: fields.next()?.to_owned(),
                    committed: fields.next()?.parse().ok()?,
                    subject: fields.next().unwrap_or_default().to_owned(),
                })
            })
            .collect())
    }

    /// Trees whose note changed between two tips of a notes ref, with the new
    /// blob (`None`: the note was removed). Paths are normalised across Git's
    /// fanout layouts (`ab/cdef…` against `abcdef…`). From a previous tip,
    /// Git lists only the entries that differ, so a catch-up after one record
    /// reads one line, not every note's.
    pub(super) fn changed_notes(
        &self,
        old: Option<&str>,
        new: &ObjectId,
    ) -> Result<Vec<NoteChange>, IndexError> {
        let (old_entries, new_entries) = match old {
            Some(old) => self.differing_entries(old, new.as_str())?,
            None => (BTreeMap::new(), self.note_entries(new.as_str())?),
        };
        let mut changed = Vec::new();
        for (object, blob) in &new_entries {
            if old_entries.get(object) != Some(blob)
                && let Ok(tree) = TreeId::parse(object)
            {
                changed.push(NoteChange {
                    tree,
                    before: old_entries.get(object).cloned(),
                    after: Some(blob.clone()),
                });
            }
        }
        for object in old_entries.keys() {
            if !new_entries.contains_key(object)
                && let Ok(tree) = TreeId::parse(object)
            {
                changed.push(NoteChange {
                    tree,
                    before: old_entries.get(object).cloned(),
                    after: None,
                });
            }
        }
        Ok(changed)
    }

    /// The note entries that differ between notes commits `old` and `new`:
    /// each side's blob, keyed by normalised path. A note Git moved between
    /// fanout layouts appears on both sides with the same blob, so it is no
    /// change.
    fn differing_entries(
        &self,
        old: &str,
        new: &str,
    ) -> Result<(NoteEntries, NoteEntries), IndexError> {
        let out = self
            .git()
            .run(&["diff-tree", "-r", "-z", "--no-renames", old, new], None)?;
        let text = String::from_utf8_lossy(&out);
        let mut tokens = text.split('\0');
        let (mut before, mut after) = (BTreeMap::new(), BTreeMap::new());
        while let Some(token) = tokens.next() {
            let Some(meta) = token.strip_prefix(':') else {
                continue;
            };
            let Some(path) = tokens.next() else {
                break;
            };
            let fields: Vec<&str> = meta.split(' ').collect();
            let blob = |field: Option<&&str>| {
                field
                    .and_then(|id| ObjectId::parse(id).ok())
                    .filter(|id| id.as_str().bytes().any(|b| b != b'0'))
            };
            let object = path.replace('/', "");
            if let Some(id) = blob(fields.get(2)) {
                before.insert(object.clone(), id);
            }
            if let Some(id) = blob(fields.get(3)) {
                after.insert(object, id);
            }
        }
        Ok((before, after))
    }

    fn note_entries(&self, commit: &str) -> Result<NoteEntries, IndexError> {
        let out = self
            .git()
            .run(&["ls-tree", "-r", "-z", "--full-tree", commit], None)?;
        let mut entries = BTreeMap::new();
        for record in String::from_utf8_lossy(&out).split('\0') {
            let Some((meta, path)) = record.split_once('\t') else {
                continue;
            };
            let id = meta
                .split(' ')
                .nth(2)
                .and_then(|id| ObjectId::parse(id).ok());
            if let Some(id) = id {
                entries.insert(path.replace('/', ""), id);
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use crate::index::fixtures::{index, record_a_run};
    use crate::testing::Scratch;

    /// Notes Git moved into its fanout layout (`ab/cdef…`) are the same
    /// notes: a catch-up across the move folds nothing again.
    #[test]
    fn notes_moved_between_fanout_layouts_are_no_change() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        record_a_run(&scratch);
        let index = index(&scratch);
        let git = index.git();
        let flat = git
            .resolve("refs/notes/evidence/perf")
            .expect("resolve")
            .expect("notes");
        let listing = git.run(&["ls-tree", flat.as_str()], None).expect("ls-tree");
        let listing = String::from_utf8(listing).expect("UTF-8");
        let (meta, name) = listing.trim_end().split_once('\t').expect("one note");
        let (head, rest) = name.split_at(2);
        let inner = git
            .run(&["mktree"], Some(format!("{meta}\t{rest}\n").as_bytes()))
            .expect("inner tree");
        let inner = String::from_utf8(inner).expect("UTF-8");
        let outer = git
            .run(
                &["mktree"],
                Some(format!("040000 tree {}\t{head}\n", inner.trim()).as_bytes()),
            )
            .expect("outer tree");
        let outer = String::from_utf8(outer).expect("UTF-8");
        let moved = git
            .run(
                &[
                    "commit-tree",
                    outer.trim(),
                    "-p",
                    flat.as_str(),
                    "-m",
                    "fanout",
                ],
                None,
            )
            .expect("commit");
        let moved =
            crate::format::id::ObjectId::parse(String::from_utf8(moved).expect("UTF-8").trim())
                .expect("id");
        let changed = index
            .changed_notes(Some(flat.as_str()), &moved)
            .expect("changes");
        assert!(changed.is_empty(), "{} changes", changed.len());
    }
}
