//! A throwaway repository for testing an integration (feature `testing`):
//! landings on a trunk, a declared machine, and the
//! [`Evidence`] over them. It never touches the caller's repository, and it is
//! removed when dropped.

use crate::evidence::{Error, Evidence};
use crate::format::id::{CommitId, Kind, TreeId};
use git_process::{Git, TreeEntry};
use std::path::Path;
use std::process::Command;

/// The note grammar's own reading of one raw note, for a producer's tests that
/// assert what landed byte for byte; answers go through [`Evidence::at`].
pub use crate::format::fold::{Answered, Folded, RunState, fold};

/// The index's file name in the store's state directory, for a test that
/// removes or inspects the disposable cache.
pub use crate::index::file_name as index_file_name;

/// How long a test waits for something that must happen before it fails: a
/// bound meant never to expire, not a claim about speed. A loaded host (a
/// whole workspace's tests at once) can take minutes over what an idle one
/// does in milliseconds, and a hang still fails.
pub const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(300);

/// The raw note of `kind` on `tree`, byte for byte, or `None` when there is
/// none: what [`fold`] reads.
///
/// # Errors
/// Git cannot be run, or fails for a reason other than a missing note.
pub fn note(evidence: &Evidence, kind: &Kind, tree: &TreeId) -> Result<Option<Vec<u8>>, Error> {
    Ok(evidence.store().read_note(kind, tree)?)
}

/// [`Evidence::audit`] with explicit person and host names (any
/// case): a test of what the audit flags that holds on every machine.
///
/// # Errors
/// As [`Evidence::audit`].
pub fn audit_as(
    evidence: &Evidence,
    person: &[&str],
    host: &[&str],
) -> Result<crate::share::audit::Audit, Error> {
    let own = crate::share::audit::OwnNames::new(
        person.iter().map(|s| (*s).to_owned()).collect(),
        host.iter().map(|s| (*s).to_owned()).collect(),
    );
    evidence.audit_against(&own)
}

/// [`Evidence::push`] with explicit person and host names: a push test that
/// holds on every machine.
///
/// # Errors
/// As [`Evidence::push`].
pub fn push_as(
    evidence: &Evidence,
    remote: &str,
    person: &[&str],
    host: &[&str],
) -> Result<crate::share::remote::Transfer, Error> {
    let own = crate::share::audit::OwnNames::new(
        person.iter().map(|s| (*s).to_owned()).collect(),
        host.iter().map(|s| (*s).to_owned()).collect(),
    );
    evidence.push_against(remote, &own)
}

/// Every raw note of `kind`, with the tree it is on, in Git's order.
///
/// # Errors
/// Git fails, or prints a listing that is not `<blob> <tree>` lines.
pub fn notes(evidence: &Evidence, kind: &Kind) -> Result<Vec<(TreeId, Vec<u8>)>, Error> {
    let git = evidence.git();
    let notes_ref = crate::store::Store::notes_ref(kind);
    if git
        .resolve(&notes_ref)
        .map_err(crate::store::StoreError::from)?
        .is_none()
    {
        return Ok(Vec::new());
    }
    let listing = git
        .run(&["notes", "--ref", &notes_ref, "list"], None)
        .map_err(crate::store::StoreError::from)?;
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&listing).lines() {
        let (blob, tree) = line.split_once(' ').ok_or_else(|| {
            crate::store::StoreError::from(git_process::GitError::Unexpected {
                args: "notes list".to_owned(),
                detail: line.to_owned(),
            })
        })?;
        let tree = TreeId::parse(tree)
            .map_err(git_process::GitError::from)
            .map_err(crate::store::StoreError::from)?;
        let blob = crate::format::id::ObjectId::parse(blob)
            .map_err(git_process::GitError::from)
            .map_err(crate::store::StoreError::from)?;
        found.push((
            tree,
            git.read_blob(&blob)
                .map_err(crate::store::StoreError::from)?,
        ));
    }
    Ok(found)
}

/// A scratch repository: `master` with landings, each with its own tree,
/// committed as `git_process::testing`'s fixture identity on every machine.
pub struct Scratch {
    dir: tempfile::TempDir,
    git: Git,
    landings: Vec<CommitId>,
}

impl Scratch {
    /// An empty repository with no landings yet.
    ///
    /// # Errors
    /// Git cannot be run, or the directory cannot be made.
    pub fn new() -> Result<Self, Error> {
        // A caller-installed observer takes precedence over fixture diagnostics.
        let _ = git_process::observe_start_failures::<git_process::StartFailureLog>();
        let dir = tempfile::tempdir().map_err(|source| crate::store::StoreError::Io {
            path: std::env::temp_dir(),
            source,
        })?;
        // Inherited GIT_DIR (in a hook, `rebase -x`, `bisect run`) would make
        // `git init` re-initialise the caller's repository instead; the
        // runner's global configuration would give it the runner's templates.
        let status = git_process::start(
            git_process::testing::hermetic(&mut Command::new("git"))
                .args(["init", "-q", "--initial-branch=master"])
                .arg(dir.path()),
        )
        .map_err(crate::store::StoreError::from)?
        .wait()
        .map_err(|source| crate::store::StoreError::Io {
            path: dir.path().to_path_buf(),
            source,
        })?;
        if !status.success() {
            return Err(crate::store::StoreError::NotFound(dir.path().to_path_buf()).into());
        }
        let git = Git::at(dir.path().join(".git"));
        // Notes commits are written by Git the store starts: they carry the
        // fixture's identity, never the runner's, and are never signed.
        git_process::testing::fixture_config(&git).map_err(crate::store::StoreError::from)?;
        Ok(Self {
            dir,
            git,
            landings: Vec::new(),
        })
    }

    /// Adds `count` landings to `master`, each with a distinct tree.
    ///
    /// # Errors
    /// A Git failure.
    pub fn landings(mut self, count: usize) -> Result<Self, Error> {
        for _ in 0..count {
            let n = self.landings.len();
            let blob = self
                .git
                .write_blob(format!("landing {n}\n").as_bytes())
                .map_err(crate::store::StoreError::from)?;
            let tree = self
                .git
                .write_tree(&[TreeEntry {
                    mode: "100644".into(),
                    kind: "blob".into(),
                    id: blob,
                    name: "landing".into(),
                }])
                .map_err(crate::store::StoreError::from)?;
            let previous = self.landings.last().map(CommitId::object);
            let parents: Vec<_> = previous.into_iter().collect();
            let commit = self
                .git
                .write_commit(&tree, &parents, &format!("landing {n}"))
                .map_err(crate::store::StoreError::from)?;
            // A compare-and-swap from the previous landing (none at first).
            self.git
                .update_ref("refs/heads/master", commit.object(), previous)
                .map_err(crate::store::StoreError::from)?;
            self.landings.push(commit);
        }
        Ok(self)
    }

    /// Declares the machine: `git config evidence.machine <name>`.
    ///
    /// # Errors
    /// A Git failure.
    pub fn machine(self, name: &str) -> Result<Self, Error> {
        self.git
            .run(&["config", "evidence.machine", name], None)
            .map_err(crate::store::StoreError::from)?;
        Ok(self)
    }

    /// The evidence of this repository, reading landings from `master`.
    #[must_use]
    pub fn evidence(&self) -> Evidence {
        Evidence::open(self.git.git_dir()).trunk("refs/heads/master")
    }

    /// The `n`th landing, oldest first.
    ///
    /// # Panics
    /// If there are not `n + 1` landings: a mistake in the test.
    #[must_use]
    pub fn landing(&self, n: usize) -> CommitId {
        self.landings[n].clone()
    }

    /// The repository's working directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The variable that points git-evidence at this store, for a child
    /// process started some other way: `(GIT_EVIDENCE_GIT_DIR, its .git)`.
    #[must_use]
    pub fn env(&self) -> (&'static str, std::path::PathBuf) {
        ("GIT_EVIDENCE_GIT_DIR", self.git.git_dir().to_path_buf())
    }

    /// `program` run in this repository, with `GIT_EVIDENCE_GIT_DIR` set,
    /// every inherited variable that redirects Git removed, and Git's
    /// configuration and identity the fixture's
    /// ([`git_process::testing::hermetic`]), so a child (the CLI, a producer
    /// running `git`) reads and writes this store, never the repository the
    /// test runs in, and behaves the same on every runner.
    #[must_use]
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let (name, value) = self.env();
        let mut command = Command::new(program);
        git_process::testing::hermetic(&mut command)
            .current_dir(self.dir.path())
            .env(name, value);
        command
    }
}
