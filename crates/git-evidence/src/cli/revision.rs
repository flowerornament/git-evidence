//! A revision named on the command line, resolved to the commit and tree the
//! API asks about, and a range `A..B` to the commits a selector names: a Git
//! revision, or inside a jj workspace a jj revset first (README "Revisions,
//! jj and the manual").

use git_evidence::{CommitId, Evidence, LandingRange, TreeId};

/// Why a revision named on the command line has no tree here.
#[derive(Debug, thiserror::Error)]
enum RevisionError {
    #[error(
        "this is a jj workspace and jj cannot run ({0}): install jj, or run from a Git checkout"
    )]
    JjUnavailable(std::io::Error),
    #[error(
        "{rev} names no commit here: jj cannot resolve it ({detail}), nor Git, reading HEAD as jj's @-"
    )]
    Unresolved { rev: String, detail: String },
    #[error("{0} names no revision here; name one that exists (HEAD, main, a commit id)")]
    NoRevision(String),
    #[error(
        "{0} would read Git's HEAD or its reflog, which in a jj workspace is another workspace's checkout: write HEAD, HEAD~N or HEAD^N (this workspace's @-), or a jj revision such as @- or a commit id"
    )]
    SharedHead(String),
    #[error("{rev} names {count} revisions; name one")]
    Ambiguous { rev: String, count: usize },
    #[error("{0} is not a range of landings: write A..B, a landing on each side")]
    Range(String),
}

/// A revision's tree, its commit resolved as [`resolve_commit`] says.
pub fn resolve_tree(evidence: &Evidence, rev: &str) -> Result<TreeId, Box<dyn std::error::Error>> {
    let commit = resolve_commit(evidence, rev)?;
    Ok(git_process::Git::at(evidence.git_dir()).tree_of(commit.object())?)
}

/// `A..B` as the two commits it names, or `None` when `rev` is not a range.
pub fn resolve_range(
    evidence: &Evidence,
    rev: &str,
) -> Result<Option<LandingRange>, Box<dyn std::error::Error>> {
    let Some((after, through)) = rev.split_once("..") else {
        return Ok(None);
    };
    if after.is_empty() || through.is_empty() || through.contains("..") {
        return Err(RevisionError::Range(rev.to_owned()).into());
    }
    Ok(Some(LandingRange {
        after: resolve_commit(evidence, after)?,
        through: resolve_commit(evidence, through)?,
    }))
}

/// A revision's commit. Outside a jj workspace it is a Git revision. Inside
/// one it is a jj revset first; one jj cannot resolve (`HEAD~2`,
/// `origin/main`) is read as a Git revision of the store, with `HEAD` meaning
/// this workspace's `@-`. The shared store's own `HEAD` and reflog name
/// another workspace's checkout, so any other spelling of them (`head`,
/// `HEAD@{0}`, `@{1}`, `@^0`) is refused, never read.
pub fn resolve_commit(
    evidence: &Evidence,
    rev: &str,
) -> Result<CommitId, Box<dyn std::error::Error>> {
    if !evidence.in_jj_workspace() {
        return git_commit(evidence, rev)?
            .ok_or_else(|| RevisionError::NoRevision(rev.to_owned()).into());
    }
    let commits = match jj_commits(rev)? {
        Ok(commits) => commits,
        Err(detail) => {
            let unresolved = || RevisionError::Unresolved {
                rev: rev.to_owned(),
                detail: detail.clone(),
            };
            let spelled = match head_named(rev) {
                Some(after) => {
                    let parent = jj_commits("@-")?.map_err(|_| unresolved())?;
                    let [parent] = parent.as_slice() else {
                        return Err(unresolved().into());
                    };
                    format!("{parent}{after}")
                }
                None if names_shared_head(rev) => {
                    return Err(RevisionError::SharedHead(rev.to_owned()).into());
                }
                None => rev.to_owned(),
            };
            return git_commit(evidence, &spelled)?.ok_or_else(|| unresolved().into());
        }
    };
    match commits.as_slice() {
        [commit] => Ok(CommitId::parse(commit)?),
        [] => Err(RevisionError::NoRevision(rev.to_owned()).into()),
        many => Err(RevisionError::Ambiguous {
            rev: rev.to_owned(),
            count: many.len(),
        }
        .into()),
    }
}

/// `rev` as a Git revision of the store, or `None` when Git names no commit.
fn git_commit(
    evidence: &Evidence,
    rev: &str,
) -> Result<Option<CommitId>, Box<dyn std::error::Error>> {
    let git = git_process::Git::at(evidence.git_dir());
    match git.resolve(&format!("{rev}^{{commit}}"))? {
        Some(commit) => Ok(Some(CommitId::parse(commit.as_str())?)),
        None => Ok(None),
    }
}

/// What follows `HEAD` in `rev` when `rev` is `HEAD` and ancestry steps
/// only (`""`, `~2`, `^`, `^2~1`), else `None`.
fn head_named(rev: &str) -> Option<&str> {
    let after = rev.strip_prefix("HEAD")?;
    after
        .chars()
        .all(|c| c == '~' || c == '^' || c.is_ascii_digit())
        .then_some(after)
}

/// Whether Git would read `rev` through the shared store's `HEAD` or a
/// reflog: any `@` (`@`, `@{1}`, `x@{0}`), and a name one of whose parts is
/// `HEAD` in any case or ends in `_HEAD` (`head`, `ORIG_HEAD`,
/// `origin/HEAD`). Other names that merely contain the letters, such as
/// `refs/heads/main`, are read.
fn names_shared_head(rev: &str) -> bool {
    let name = rev.split(['~', '^', ':']).next().unwrap_or(rev);
    rev.contains('@')
        || name.split('/').any(|part| {
            let part = part.to_ascii_lowercase();
            part == "head" || part.ends_with("_head")
        })
}

/// The commits jj resolves `rev` to, or jj's own refusal.
fn jj_commits(rev: &str) -> Result<Result<Vec<String>, String>, RevisionError> {
    let output = std::process::Command::new("jj")
        .args([
            "log",
            "--no-graph",
            "--ignore-working-copy",
            "-r",
            rev,
            "-T",
            "commit_id ++ \"\\n\"",
        ])
        .output()
        .map_err(RevisionError::JjUnavailable)?;
    if !output.status.success() {
        return Ok(Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()));
    }
    Ok(Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()))
}
