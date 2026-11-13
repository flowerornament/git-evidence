//! The store a bench measures: which one ([`BenchStore`]), the size of a
//! generated one, and how it is made: an empty bare repository of its own,
//! and for an existing store a copy that only reads it (Git alternates for
//! its objects, its evidence refs and its trunk copied).

use super::BenchError;
use crate::evidence::Evidence;
use crate::store::StoreError;
use git_process::Git;
use serde::Serialize;
use std::num::NonZeroU32;
use std::path::Path;
use std::process::{Command, Stdio};

/// The size of a generated store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StoreSize {
    /// Landings on its trunk.
    pub landings: NonZeroU32,
    /// Runs recorded on them, spread evenly; half perf, half checks.
    pub runs: u32,
}

impl StoreSize {
    /// The size git-evidence's speed budgets are declared at: 5,000 landings
    /// and 10,000 runs.
    pub const TARGET: Self = Self {
        landings: NonZeroU32::new(5000).expect("nonzero"),
        runs: 10_000,
    };
}

/// The store a bench measures.
#[derive(Debug, Clone, Copy)]
pub enum BenchStore<'a> {
    /// A fresh store of this size, the same on every machine.
    Generated(StoreSize),
    /// A copy of this store's evidence and trunk, sharing its objects.
    Existing(&'a Evidence),
}

/// Who commits to a bench's own store: the bench, the same on every machine,
/// never the person or seat running it.
pub(super) const COMMITTER_NAME: &str = "bench";
/// The address the bench commits as.
pub(super) const COMMITTER_EMAIL: &str = "bench@example.invalid";

/// An empty bare repository at `path`, never the caller's: inherited
/// variables that redirect Git are removed (as `testing::Scratch` does). Its
/// own configuration names the bench as who commits, so `record` measured in
/// it runs on a machine with no identity of its own, or one that signs.
pub(super) fn init_bare(path: &Path) -> Result<Git, BenchError> {
    let status = git_process::start(
        git_process::isolated(&mut Command::new("git"))
            .args(["init", "-q", "--bare"])
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?
    .wait()
    .map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !status.success() {
        return Err(StoreError::NotFound(path.to_path_buf()).into());
    }
    let git = Git::at(path);
    for (key, value) in [
        ("user.name", COMMITTER_NAME),
        ("user.email", COMMITTER_EMAIL),
        ("commit.gpgSign", "false"),
    ] {
        git.run(&["config", "--local", key, value], None)?;
    }
    Ok(git)
}

/// Makes `copy` a copy of `evidence`'s store for reading: its objects through
/// Git alternates, its evidence refs and its trunk (as `refs/heads/trunk`,
/// which it returns). The store copied is only read.
pub(super) fn copy(evidence: &Evidence, copy: &Git) -> Result<String, BenchError> {
    const TRUNK: &str = "refs/heads/trunk";
    let source = Git::at(evidence.git_dir());
    let objects = source.run(
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
        None,
    )?;
    let alternates = copy.git_dir().join("objects/info/alternates");
    std::fs::write(&alternates, objects).map_err(|source| StoreError::Io {
        path: alternates,
        source,
    })?;
    let trunk = evidence.trunk_ref()?;
    let tip = source
        .resolve(&format!("{trunk}^{{commit}}"))?
        .ok_or(StoreError::NoTrunk)?;
    let mut updates = format!("update {TRUNK} {}\n", tip.as_str());
    let refs = source.run(
        &[
            "for-each-ref",
            "--format=update %(refname) %(objectname)",
            "refs/notes/evidence/",
        ],
        None,
    )?;
    updates.push_str(&String::from_utf8_lossy(&refs));
    copy.run(&["update-ref", "--stdin"], Some(updates.as_bytes()))?;
    Ok(TRUNK.to_owned())
}
