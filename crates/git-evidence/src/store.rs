//! The store: the Git directory where evidence lives, as notes on trees under
//! one ref per kind. [`Store`] is the handle every part of the tool shares:
//! writing appends to notes, the index folds them, sharing fetches and pushes
//! the refs, and the bench reads a copy.
//!
//! ```text
//!    main:     C1 ───── C2 ───── C3             one notes ref per kind;
//!                       │        │              each note is on a tree,
//!                    [note]   [note]            so identical code shares its evidence
//! ```
//!
//! One notes ref per kind, `refs/notes/evidence/<kind>`, with notes keyed by tree
//! id in Git's own fanout layout, so an append costs the same at any size. Lines
//! are only ever appended; another store's lines arrive by union merge
//! ([`crate::share::remote`]). Within one store, writers of a kind serialise on a lock.

use crate::format::id::{Kind, TreeId};
use git_process::{Git, GitError};
use std::fs::File;
use std::path::{Path, PathBuf};

/// A store-level failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("no Git store found from {0}: run inside a Git or jj repository")]
    NotFound(PathBuf),
    /// The nearest repository boundary is not one evidence can read. Discovery
    /// never looks past it: an outer repository would answer about another tree.
    #[error(
        "the repository at {workspace} has no Git store evidence can read ({why}): repair it, or name the store with GIT_EVIDENCE_GIT_DIR"
    )]
    Unreadable { workspace: PathBuf, why: String },
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("no trunk found: name one with --trunk (origin/HEAD, main and master are absent)")]
    NoTrunk,
    #[error("an event line must be one line of JSON")]
    Multiline,
    #[error("this store is a {0} clone: evidence needs its full history and note blobs; {1}")]
    Incomplete(&'static str, &'static str),
    #[error(
        "this store names no machine: run `git --git-dir <store> config --local evidence.machine <name>` (bench, laptop, …)"
    )]
    NoMachine,
    #[error("Git config {name} is {value:?}, not a byte count: set it to a whole number of bytes")]
    BadBudget { name: String, value: String },
    #[error(
        "cannot read Git config {name} ({detail}): leave exactly one value, in a readable config file"
    )]
    BudgetUnreadable { name: String, detail: String },
    #[error(
        "cannot read Git config evidence.machine ({0}): leave exactly one value, in a readable config file"
    )]
    MachineUnreadable(String),
    #[error(
        "Git config evidence.machine is not a machine name ({0}): run `git config evidence.machine <name>`"
    )]
    BadMachine(crate::format::id::IdError),
}

/// Where every kind's notes ref lives: `refs/notes/evidence/<kind>`.
pub const NOTES: &str = "refs/notes/evidence/";

/// Where an append stages a kind's notes before its ref moves:
/// `refs/notes/evidence-staging/<kind>`. Never pushed, fetched or read.
pub const STAGING: &str = "refs/notes/evidence-staging/";

/// Where `fetch` files a remote's copies: `refs/notes/remotes/<remote>/evidence/<kind>`.
/// Under `refs/notes/`, so Git and jj never mistake them for branches.
#[must_use]
pub fn fetched_notes_prefix(remote: &str) -> String {
    format!("refs/notes/remotes/{remote}/evidence/")
}

/// A handle on one store: its Git directory, found from a working directory
/// or named, with its notes refs, settings and locks.
#[derive(Debug, Clone)]
pub struct Store {
    git: Git,
    /// Whether discovery found this store through a jj workspace: there the
    /// working copy is jj's, and the store's Git refs name another checkout.
    jj: bool,
}

impl Store {
    /// Opens the store with this Git directory.
    #[must_use]
    pub fn open(git_dir: impl Into<PathBuf>) -> Self {
        Self {
            git: Git::at(git_dir),
            jj: false,
        }
    }

    /// Finds the shared Git store from `start`: the nearest directory holding
    /// `.jj` or `.git` is the repository, and discovery stops there. A jj
    /// workspace's `.jj/repo` pointer and its store's `git_target` lead to the
    /// Git store; a `.git` file names a worktree's. `GIT_EVIDENCE_GIT_DIR`
    /// overrides discovery.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when no repository encloses `start`;
    /// [`StoreError::Unreadable`] when the nearest one has no Git store this
    /// can read, never an answer from a repository further out.
    pub fn discover(start: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = std::env::var_os("GIT_EVIDENCE_GIT_DIR") {
            return Ok(Self::open(PathBuf::from(dir)));
        }
        for dir in start.ancestors() {
            if dir.join(".jj").exists() {
                return Self::jj_store(dir).map(|store| Self {
                    jj: true,
                    ..Self::open(store)
                });
            }
            if dir.join(".git").exists() {
                return Self::git_store(dir).map(Self::open);
            }
        }
        Err(StoreError::NotFound(start.to_path_buf()))
    }

    /// Whether this store was found through a jj workspace.
    #[must_use]
    pub fn in_jj_workspace(&self) -> bool {
        self.jj
    }

    /// The Git store of the jj workspace at `workspace`.
    fn jj_store(workspace: &Path) -> Result<PathBuf, StoreError> {
        let unreadable = |why: &str| StoreError::Unreadable {
            workspace: workspace.to_path_buf(),
            why: why.to_owned(),
        };
        let pointer = workspace.join(".jj").join("repo");
        let jj_repo = if pointer.is_file() {
            workspace.join(".jj").join(read_trimmed(&pointer)?)
        } else if pointer.is_dir() {
            pointer
        } else {
            return Err(unreadable(".jj has no repo"));
        };
        let target = jj_repo.join("store").join("git_target");
        if !target.is_file() {
            return Err(unreadable("its jj store is not backed by Git"));
        }
        canonical(&jj_repo.join("store").join(read_trimmed(&target)?))
    }

    /// The Git store of the repository whose `.git` is at `workspace`.
    fn git_store(workspace: &Path) -> Result<PathBuf, StoreError> {
        let dot_git = workspace.join(".git");
        if dot_git.is_dir() {
            return canonical(&dot_git);
        }
        let text = read_trimmed(&dot_git)?;
        let Some(path) = text.strip_prefix("gitdir: ") else {
            return Err(StoreError::Unreadable {
                workspace: workspace.to_path_buf(),
                why: ".git is a file that names no gitdir".into(),
            });
        };
        let worktree_git = workspace.join(path);
        let common = worktree_git.join("commondir");
        let git_dir = if common.is_file() {
            worktree_git.join(read_trimmed(&common)?)
        } else {
            worktree_git
        };
        canonical(&git_dir)
    }

    /// The trunk: the ref whose first-parent line is the landings. `origin/HEAD`'s
    /// target when the clone records one, else the first of `origin/main`,
    /// `origin/master`, `main`, `master` that exists.
    ///
    /// # Errors
    /// Refuses when Git fails, or when none of those refs exists.
    pub fn default_trunk(&self) -> Result<String, StoreError> {
        let head = self
            .git
            .output(&["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])?;
        if head.status.success() {
            let target = String::from_utf8_lossy(&head.stdout).trim().to_owned();
            // After a rename and prune origin/HEAD can name a ref that is gone.
            if !target.is_empty() && self.git.resolve(&target)?.is_some() {
                return Ok(target);
            }
        }
        for candidate in [
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
            "refs/heads/main",
            "refs/heads/master",
        ] {
            if self.git.resolve(candidate)?.is_some() {
                return Ok(candidate.to_owned());
            }
        }
        Err(StoreError::NoTrunk)
    }

    #[must_use]
    pub fn git(&self) -> &Git {
        &self.git
    }

    /// The notes ref of one kind.
    #[must_use]
    pub fn notes_ref(kind: &Kind) -> String {
        format!("{NOTES}{kind}")
    }

    /// The machine this store's producers run on: the store's own (`--local`)
    /// Git config `evidence.machine`, exactly one value. A user-wide value is
    /// never read, so one machine's setting cannot leak into another store. No
    /// code path reads a hostname; importers name their machine explicitly.
    ///
    /// # Errors
    /// [`StoreError::NoMachine`], [`StoreError::BadMachine`] or
    /// [`StoreError::MachineUnreadable`], each naming the
    /// fix; a Git failure.
    pub fn machine(&self) -> Result<crate::format::id::Machine, StoreError> {
        let configured =
            self.git
                .output(&["config", "--local", "--get-all", "evidence.machine"])?;
        // Exit 1 means unset. Several values, or a config Git cannot read,
        // cannot say which machine this is: that is not "unset".
        match configured.status.code() {
            Some(0) => {}
            Some(1) => return Err(StoreError::NoMachine),
            _ => {
                return Err(StoreError::MachineUnreadable(
                    String::from_utf8_lossy(&configured.stderr)
                        .trim()
                        .to_owned(),
                ));
            }
        }
        let values = String::from_utf8(configured.stdout)
            .map_err(|_| StoreError::MachineUnreadable("the value is not UTF-8".to_owned()))?;
        let values: Vec<&str> = values.lines().collect();
        let [name] = values.as_slice() else {
            return Err(StoreError::MachineUnreadable(format!(
                "{} values: {}",
                values.len(),
                values.join(", ")
            )));
        };
        let name = (*name).to_owned();
        crate::format::id::Machine::parse(&name).map_err(StoreError::BadMachine)
    }

    /// The size budget of one run of `kind`, in bytes: the store's
    /// `evidence.<kind>.budget` (a subsection, so every kind name is a valid
    /// key). `None` while unset: budgets are set from measured runs, never
    /// estimated.
    ///
    /// # Errors
    /// [`StoreError::BadBudget`] for a value that is not a byte count, and
    /// [`StoreError::BudgetUnreadable`] when Git cannot say (several values,
    /// an unreadable config): that is never read as "no budget".
    pub fn budget(&self, kind: &Kind) -> Result<Option<u64>, StoreError> {
        let name = format!("evidence.{kind}.budget");
        let configured = self
            .git
            .output(&["config", "--local", "--get-all", &name])?;
        match configured.status.code() {
            Some(0) => {}
            Some(1) => return Ok(None),
            _ => {
                return Err(StoreError::BudgetUnreadable {
                    name,
                    detail: String::from_utf8_lossy(&configured.stderr)
                        .trim()
                        .to_owned(),
                });
            }
        }
        let text = String::from_utf8_lossy(&configured.stdout)
            .trim()
            .to_owned();
        if text.lines().count() > 1 {
            return Err(StoreError::BudgetUnreadable {
                name,
                detail: format!("several values: {}", text.replace('\n', ", ")),
            });
        }
        text.parse::<u64>()
            .map(Some)
            .map_err(|_| StoreError::BadBudget { name, value: text })
    }

    /// Refuses a shallow or partial clone: an index over one would be missing
    /// history or fetch every note blob one by one.
    ///
    /// # Errors
    /// [`StoreError::Incomplete`], or a Git failure.
    pub fn require_complete(&self) -> Result<(), StoreError> {
        let shallow = self
            .git
            .run(&["rev-parse", "--is-shallow-repository"], None)?;
        if String::from_utf8_lossy(&shallow).trim() == "true" {
            return Err(StoreError::Incomplete("shallow", "fetch with --unshallow"));
        }
        let partial = self
            .git
            .output(&["config", "--get", "remote.origin.promisor"])?;
        if partial.status.success() && String::from_utf8_lossy(&partial.stdout).trim() == "true" {
            return Err(StoreError::Incomplete(
                "partial",
                "clone without --filter, or fetch the missing objects first",
            ));
        }
        Ok(())
    }

    /// The directory for this crate's local state inside the Git directory.
    #[must_use]
    pub fn state_dir(&self) -> PathBuf {
        self.git.git_dir().join("evidence")
    }

    /// Takes the local lock for one notes ref, held until the guard drops: writers
    /// in one store serialise on it.
    ///
    /// # Errors
    /// Refuses when the lock file cannot be created or locked.
    pub fn lock(&self, notes_ref: &str) -> Result<RefLock, StoreError> {
        let dir = self.state_dir().join("locks");
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join(format!("{}.lock", notes_ref.replace('/', "%")));
        let file = File::create(&path).map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
        file.lock()
            .map_err(|source| StoreError::Io { path, source })?;
        Ok(RefLock { _file: file })
    }

    /// The note of `kind` on `tree`, if any.
    ///
    /// # Errors
    /// Refuses when Git fails for a reason other than a missing note.
    pub fn read_note(&self, kind: &Kind, tree: &TreeId) -> Result<Option<Vec<u8>>, StoreError> {
        let notes_ref = Self::notes_ref(kind);
        // Not `git notes show`: it hands the note to a second Git (`git
        // show`), and when that one cannot start it exits 1 with nothing on
        // stderr, as for a missing note. `list` names the note's blob, and
        // exits 1 only when there is none.
        let args = ["notes", "--ref", &notes_ref, "list", tree.as_str()];
        let output = self.git.output(&args)?;
        match output.status.code() {
            Some(0) => {
                let blob = crate::format::id::ObjectId::parse(
                    String::from_utf8_lossy(&output.stdout).trim(),
                )
                .map_err(GitError::from)?;
                Ok(Some(
                    self.git.run(&["cat-file", "blob", blob.as_str()], None)?,
                ))
            }
            Some(1) => Ok(None),
            _ => Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }
            .into()),
        }
    }

    /// Appends each tree's lines to its note of `kind`, all or nothing. One
    /// note is appended on the kind's ref directly: Git moves the ref in one
    /// atomic update. Several are written on a staging ref copied from the
    /// kind's ref, which then moves to the result with one compare-and-swap.
    /// The kind's ref must be held with [`Store::lock`], so no other writer
    /// moves it meanwhile. Returns after the objects, the ref and the ref's
    /// directory are on disk. A staging ref left by a crash is harmless; the
    /// next append of several starts it afresh.
    ///
    /// # Errors
    /// Refuses multi-line input, and fails when Git or the disk does.
    pub fn append(
        &self,
        _lock: &RefLock,
        kind: &Kind,
        notes: &[(TreeId, Vec<Vec<u8>>)],
    ) -> Result<(), StoreError> {
        if notes
            .iter()
            .flat_map(|(_, lines)| lines)
            .any(|line| line.contains(&b'\n'))
        {
            return Err(StoreError::Multiline);
        }
        let notes_ref = Self::notes_ref(kind);
        if let [(tree, lines)] = notes {
            self.append_note(&notes_ref, tree, lines)?;
            crate::write::fault::point("append:ref-moved");
            return self.harden_ref(&notes_ref);
        }
        let staging = format!("{STAGING}{kind}");
        let before = self.git.resolve(&notes_ref)?;
        match &before {
            Some(tip) => self
                .git
                .run(&["update-ref", &staging, tip.as_str()], None)?,
            None => self.git.run(&["update-ref", "-d", &staging], None)?,
        };
        for (tree, lines) in notes {
            self.append_note(&staging, tree, lines)?;
            crate::write::fault::point("append:staged");
        }
        let Some(after) = self.git.resolve(&staging)? else {
            return Ok(()); // nothing to append
        };
        self.git.update_ref(&notes_ref, &after, before.as_ref())?;
        crate::write::fault::point("append:ref-moved");
        self.harden_ref(&notes_ref)?;
        self.git.run(&["update-ref", "-d", &staging], None)?;
        Ok(())
    }

    /// Appends `lines` to `tree`'s note on `notes_ref`, as they are: no
    /// separator, no whitespace cleanup.
    fn append_note(
        &self,
        notes_ref: &str,
        tree: &TreeId,
        lines: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        let mut body = Vec::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
        for line in lines {
            body.extend_from_slice(line);
            body.push(b'\n');
        }
        self.git.run(
            &[
                "notes",
                "--ref",
                notes_ref,
                "append",
                "--no-separator",
                "--no-stripspace",
                "-F",
                "-",
                tree.as_str(),
            ],
            Some(&body),
        )?;
        Ok(())
    }

    /// Flushes the directory holding a loose ref, so the rename that moved it
    /// survives power loss (`F_FULLFSYNC` on macOS through `sync_all`).
    fn harden_ref(&self, reference: &str) -> Result<(), StoreError> {
        // Every directory from the ref's up to the git dir: the first append
        // creates `refs/notes/evidence/`, whose own entry must survive too.
        let git_dir = self.git.git_dir();
        let path = git_dir.join(reference);
        let dirs = path
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(git_dir));
        for target in dirs {
            // Under the reftable backend a ref has no directory of its own.
            if !target.is_dir() {
                continue;
            }
            let handle = File::open(target).map_err(|source| StoreError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            handle.sync_all().map_err(|source| StoreError::Io {
                path: target.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }
}

/// A held lock on one notes ref; dropping it releases the lock.
#[derive(Debug)]
pub struct RefLock {
    _file: File,
}

fn read_trimmed(path: &Path) -> Result<String, StoreError> {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn canonical(path: &Path) -> Result<PathBuf, StoreError> {
    path.canonicalize().map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_process::testing;

    /// A directory tree for discovery: `outer` is a Git repository, and
    /// `outer/workspace` is where the caller stands.
    fn nested() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().expect("tempdir");
        let outer = root.path().to_path_buf();
        std::fs::create_dir_all(outer.join(".git")).expect("outer git");
        std::fs::create_dir_all(outer.join("workspace/src")).expect("workspace");
        (root, outer.join("workspace"))
    }

    #[test]
    fn a_workspace_without_its_own_store_refuses_and_never_answers_about_an_outer_repository() {
        // A jj workspace whose store cannot be read stops at its own
        // boundary; the outer repository (a home directory's) is never found.
        let (root, workspace) = nested();
        std::fs::create_dir_all(workspace.join(".jj")).expect("bare .jj");
        let outer_store = root.path().join(".git");
        for start in [workspace.clone(), workspace.join("src")] {
            match Store::discover(&start) {
                Err(StoreError::Unreadable {
                    workspace: named, ..
                }) => assert_eq!(named, workspace),
                Ok(store) => panic!("answered about {}", store.git.git_dir().display()),
                Err(other) => panic!("{other}"),
            }
        }
        std::fs::create_dir_all(workspace.join(".jj/repo/store")).expect("jj store");
        assert!(matches!(
            Store::discover(&workspace),
            Err(StoreError::Unreadable { .. })
        ));
        // A jj workspace backed by Git finds its own store.
        let own = root.path().join("own.git");
        std::fs::create_dir_all(&own).expect("own store");
        std::fs::write(
            workspace.join(".jj/repo/store/git_target"),
            own.to_str().expect("utf-8"),
        )
        .expect("target");
        let found = Store::discover(&workspace.join("src")).expect("own store");
        assert_eq!(found.git.git_dir(), own.canonicalize().expect("canonical"));
        assert_ne!(
            found.git.git_dir(),
            outer_store.canonicalize().expect("canonical")
        );
        // A .git file that names nothing refuses too.
        std::fs::remove_dir_all(workspace.join(".jj")).expect("unjj");
        std::fs::write(workspace.join(".git"), "not a gitdir").expect(".git file");
        assert!(matches!(
            Store::discover(&workspace),
            Err(StoreError::Unreadable { .. })
        ));
    }

    #[test]
    fn appended_lines_accumulate_on_a_trees_note_through_git_notes() {
        let (_dir, git) = testing::repo();
        let store = Store::open(git.git_dir());
        let kind = Kind::parse("perf").expect("kind");
        let tree = git.write_tree(&[]).expect("tree");
        let lock = store.lock(&Store::notes_ref(&kind)).expect("lock");
        let tree = TreeId::parse(tree.as_str()).expect("tree");
        let note = |lines: &[&str]| {
            vec![(
                tree.clone(),
                lines.iter().map(|l| l.as_bytes().to_vec()).collect(),
            )]
        };
        store
            .append(&lock, &kind, &note(&["{\"a\":1}"]))
            .expect("first");
        store
            .append(&lock, &kind, &note(&["{\"b\":2}", "{\"c\":3}"]))
            .expect("second");
        assert_eq!(
            store.read_note(&kind, &tree).expect("read"),
            Some(b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n".to_vec())
        );
        assert!(store.append(&lock, &kind, &note(&["x\ny"])).is_err());
        let other = Kind::parse("gates").expect("kind");
        assert_eq!(store.read_note(&other, &tree).expect("read"), None);
    }

    #[test]
    fn a_note_git_cannot_read_is_an_error_not_an_absent_note() {
        // A writer that read "no note" here would judge its batch against an
        // empty note; a reader would report a run that is recorded as missing.
        let (_dir, git) = testing::repo();
        let store = Store::open(git.git_dir());
        let kind = Kind::parse("perf").expect("kind");
        let tree = TreeId::parse(git.write_tree(&[]).expect("tree").as_str()).expect("tree");
        let blob = git.write_blob(b"not a notes commit").expect("blob");
        git.run(
            &["update-ref", &Store::notes_ref(&kind), blob.as_str()],
            None,
        )
        .expect("point the notes ref at a blob");
        assert!(
            matches!(store.read_note(&kind, &tree), Err(StoreError::Git(_))),
            "{:?}",
            store.read_note(&kind, &tree)
        );
    }

    const READ_PROBE: &str = "GIT_EVIDENCE_READ_NOTE_PROBE";

    #[test]
    fn a_note_is_read_without_starting_a_second_git() {
        // A loaded host can refuse to start a process. A second Git that does
        // not start (here: one that only fails) must not turn a recorded note
        // into a missing one.
        let kind = Kind::parse("perf").expect("kind");
        if let Some(store) = std::env::var_os(READ_PROBE) {
            let store = Store::open(PathBuf::from(store));
            let tree = TreeId::parse(EMPTY_TREE).expect("tree");
            assert_eq!(
                store.read_note(&kind, &tree).expect("read"),
                Some(b"{\"a\":1}\n".to_vec())
            );
            return;
        }
        let (dir, git) = testing::repo();
        let store = Store::open(git.git_dir());
        let tree = TreeId::parse(git.write_tree(&[]).expect("tree").as_str()).expect("tree");
        assert_eq!(tree.as_str(), EMPTY_TREE);
        let lock = store.lock(&Store::notes_ref(&kind)).expect("lock");
        store
            .append(&lock, &kind, &[(tree, vec![b"{\"a\":1}".to_vec()])])
            .expect("append");
        // Git looks in its exec path first for any Git it starts.
        let exec_path = dir.path().join("exec-path");
        std::fs::create_dir(&exec_path).expect("exec path");
        std::fs::write(exec_path.join("git"), "#!/bin/sh\nexit 1\n").expect("failing git");
        std::fs::set_permissions(
            exec_path.join("git"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("executable");
        let probe = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "store::tests::a_note_is_read_without_starting_a_second_git",
                "--nocapture",
            ])
            .env(READ_PROBE, git.git_dir())
            .env("GIT_EXEC_PATH", &exec_path)
            .output()
            .expect("probe");
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stdout)
        );
    }

    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    #[test]
    fn discovery_follows_a_jj_workspace_pointer_to_the_shared_store() {
        let (dir, git) = testing::repo();
        let anchor = dir.path();
        std::fs::create_dir_all(anchor.join(".jj/repo/store")).expect("jj repo");
        std::fs::write(anchor.join(".jj/repo/store/git_target"), "../../../.git").expect("target");
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".jj")).expect("workspace jj");
        let pointer = anchor.join(".jj/repo").canonicalize().expect("abs");
        std::fs::write(
            workspace.path().join(".jj/repo"),
            pointer.to_str().expect("utf8"),
        )
        .expect("pointer");
        std::fs::create_dir_all(workspace.path().join("sub/dir")).expect("nested");
        let found = Store::discover(&workspace.path().join("sub/dir")).expect("discovered");
        assert_eq!(
            found.git().git_dir(),
            git.git_dir().canonicalize().expect("abs").as_path()
        );
    }

    #[test]
    fn the_machine_is_the_stores_setting_and_never_guessed() {
        let (_dir, git) = testing::repo();
        let store = Store::open(git.git_dir());
        let set = |value: &str| {
            assert!(
                git.output(&["config", "evidence.machine", value])
                    .expect("git config")
                    .status
                    .success()
            );
        };
        assert!(matches!(store.machine(), Err(StoreError::NoMachine)));
        assert!(
            store
                .machine()
                .expect_err("unset")
                .to_string()
                .contains("config --local evidence.machine")
        );
        for bad in ["bench lab", "racks/bench", &"x".repeat(65)] {
            set(bad);
            assert!(
                matches!(store.machine(), Err(StoreError::BadMachine(_))),
                "{bad:?}"
            );
        }
        set("bench");
        assert_eq!(store.machine().expect("set").as_str(), "bench");
        // Two values cannot say which machine this is: not "unset".
        assert!(
            git.output(&["config", "--add", "evidence.machine", "laptop"])
                .expect("git config")
                .status
                .success()
        );
        assert!(matches!(
            store.machine(),
            Err(StoreError::MachineUnreadable(_))
        ));
    }
}
