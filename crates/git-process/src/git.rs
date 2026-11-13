//! The Git CLI boundary. [`Git`] applies one invocation prefix to every
//! command: no automatic gc or maintenance, objects and references flushed to
//! disk before a command returns, a fixed locale so errors can be read, and no
//! prompt that would wait on a person, so a write is on disk when the
//! command returns. The prefix is copied from git-perf. [`Git::read_blobs`]
//! batches [`Git::read_blob`] without changing its bytes.

use crate::{CommitId, IdError, ObjectId, TreeId, output_of, start};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A failed Git invocation.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// `git` never started: refused at once when missing or forbidden, else
    /// after a few tries spaced out.
    #[error("cannot run git: {0}")]
    Spawn(#[source] std::io::Error),
    /// Writing to, reading from or waiting for a `git` that started failed, or
    /// the thread that feeds it its input could not start.
    #[error("talking to git failed: {0}")]
    Io(#[source] std::io::Error),
    /// `git` exited with failure.
    #[error("git {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    /// `git` printed something that is not what the command promises.
    #[error("git {args} printed an unexpected answer: {detail}")]
    Unexpected { args: String, detail: String },
    /// A compare-and-swap on a ref lost: the ref moved since it was read.
    #[error("ref {reference} moved: expected {expected}")]
    RefMoved { reference: String, expected: String },
}

impl From<IdError> for GitError {
    fn from(error: IdError) -> Self {
        Self::Unexpected {
            args: String::new(),
            detail: error.to_string(),
        }
    }
}

/// One entry of a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub kind: String,
    pub id: ObjectId,
    pub name: String,
}

/// A Git repository, addressed by its Git directory (never a working copy).
#[derive(Debug, Clone)]
pub struct Git {
    git_dir: PathBuf,
}

/// Environment variables that would redirect Git away from `git_dir`.
const REDIRECTING: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
];

/// `command` with every variable that would redirect Git away from the
/// repository it names removed, so a child cannot reach the caller's store.
pub fn isolated(command: &mut std::process::Command) -> &mut std::process::Command {
    for name in REDIRECTING {
        command.env_remove(name);
    }
    command
}

impl Git {
    #[must_use]
    pub fn at(git_dir: impl Into<PathBuf>) -> Self {
        Self {
            git_dir: git_dir.into(),
        }
    }

    #[must_use]
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        // Never wait on a person: a remote that wants credentials refuses
        // (SSH's own prompts: `transport`).
        isolated(&mut command)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("--git-dir")
            .arg(&self.git_dir)
            .args([
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=0",
                "-c",
                "core.fsync=objects,reference",
                "-c",
                "core.fsyncMethod=fsync",
            ])
            .args(args);
        command
    }

    /// Runs `git args`, feeding `stdin`, and returns its standard output.
    ///
    /// # Errors
    /// Refuses when Git cannot start or exits with failure.
    pub fn run(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, GitError> {
        let mut child = start(
            self.command(args)
                .stdin(if stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
        if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(bytes).map_err(GitError::Io)?;
        }
        let output = child.wait_with_output().map_err(GitError::Io)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(GitError::Failed {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    /// [`Git::output`] for a command that talks to a remote (`push`, `fetch`).
    /// SSH asks on the terminal for a passphrase or an unknown host key, which
    /// no prompt setting of Git's stops; unless an SSH command is configured,
    /// it runs in batch mode and refuses instead of waiting on a person.
    ///
    /// # Errors
    /// Refuses only when Git cannot start.
    pub fn transport(&self, args: &[&str]) -> Result<std::process::Output, GitError> {
        let configured = std::env::var_os("GIT_SSH_COMMAND").is_some()
            || std::env::var_os("GIT_SSH").is_some()
            || self
                .output(&["config", "--get", "core.sshCommand"])?
                .status
                .success();
        let mut command = self.command(args);
        if !configured {
            command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        output_of(command.stdin(Stdio::null()))
    }

    /// Runs `git args` and returns its exit status and output, failure included:
    /// for commands whose failure output is itself the answer (`push`, `fetch`).
    ///
    /// # Errors
    /// Refuses only when Git cannot start.
    pub fn output(&self, args: &[&str]) -> Result<std::process::Output, GitError> {
        output_of(self.command(args).stdin(Stdio::null()))
    }

    fn run_text(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, GitError> {
        let bytes = self.run(args, stdin)?;
        String::from_utf8(bytes).map_err(|error| GitError::Unexpected {
            args: args.join(" "),
            detail: error.to_string(),
        })
    }

    /// The object a ref points at, or `None` when the ref does not exist.
    ///
    /// # Errors
    /// Refuses when Git fails for a reason other than a missing ref.
    pub fn resolve(&self, reference: &str) -> Result<Option<ObjectId>, GitError> {
        let output = output_of(
            self.command(&["rev-parse", "-q", "--verify", "--end-of-options", reference])
                .stdin(Stdio::null()),
        )?;
        match output.status.code() {
            Some(0) => {
                let text = String::from_utf8_lossy(&output.stdout);
                Ok(Some(ObjectId::parse(text.trim())?))
            }
            Some(1) => Ok(None),
            _ => Err(GitError::Failed {
                args: format!("rev-parse --verify {reference}"),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }),
        }
    }

    /// Writes `bytes` as a blob and returns its id.
    ///
    /// # Errors
    /// Refuses when Git fails.
    #[cfg(any(test, feature = "testing"))]
    pub fn write_blob(&self, bytes: &[u8]) -> Result<ObjectId, GitError> {
        let text = self.run_text(&["hash-object", "-w", "--stdin"], Some(bytes))?;
        Ok(ObjectId::parse(text.trim())?)
    }

    /// Reads a blob.
    ///
    /// # Errors
    /// Refuses when the object is missing or is not a blob.
    pub fn read_blob(&self, id: &ObjectId) -> Result<Vec<u8>, GitError> {
        self.run(&["cat-file", "blob", id.as_str()], None)
    }

    /// Reads blobs in request order, including repeated IDs and empty bodies.
    /// No partial answer escapes when any object or response is invalid.
    ///
    /// # Errors
    /// Refuses missing or non-blob objects, malformed answers, and I/O failures.
    pub fn read_blobs(&self, ids: &[ObjectId]) -> Result<Vec<Vec<u8>>, GitError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut request = Vec::new();
        for id in ids {
            writeln!(request, "{id}").map_err(GitError::Io)?;
        }
        let mut child = start(
            self.command(&["cat-file", "--batch"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
        let mut input = child.stdin.take().expect("requested piped Git input");
        // Drain answers while writing requests: either stream can exceed a pipe.
        let writer = match std::thread::Builder::new()
            .name("git-blob-requests".into())
            .spawn(move || input.write_all(&request))
        {
            Ok(writer) => writer,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::Io(error));
            }
        };
        let output = child.wait_with_output();
        let written = writer.join().map_err(|_| GitError::Unexpected {
            args: "cat-file --batch".into(),
            detail: "request writer panicked".into(),
        })?;
        written.map_err(GitError::Io)?;
        let output = output.map_err(GitError::Io)?;
        if !output.status.success() {
            return Err(GitError::Failed {
                args: "cat-file --batch".into(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        blob_answers(ids, &output.stdout)
    }

    /// The entries of a tree, not recursing.
    ///
    /// # Errors
    /// Refuses when the tree is missing or Git prints something unparseable.
    pub fn read_tree(&self, tree: &ObjectId) -> Result<Vec<TreeEntry>, GitError> {
        let text = self.run_text(&["ls-tree", "-z", tree.as_str()], None)?;
        text.split('\0')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let unexpected = || GitError::Unexpected {
                    args: "ls-tree".into(),
                    detail: line.to_owned(),
                };
                let (meta, name) = line.split_once('\t').ok_or_else(unexpected)?;
                let mut parts = meta.split(' ');
                let (Some(mode), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next())
                else {
                    return Err(unexpected());
                };
                Ok(TreeEntry {
                    mode: mode.to_owned(),
                    kind: kind.to_owned(),
                    id: ObjectId::parse(id)?,
                    name: name.to_owned(),
                })
            })
            .collect()
    }

    /// Writes a tree from entries and returns its id.
    ///
    /// # Errors
    /// Refuses when Git rejects the entries.
    #[cfg(any(test, feature = "testing"))]
    pub fn write_tree(&self, entries: &[TreeEntry]) -> Result<TreeId, GitError> {
        let mut input = Vec::new();
        for entry in entries {
            input.extend_from_slice(
                format!("{} {} {}\t{}", entry.mode, entry.kind, entry.id, entry.name).as_bytes(),
            );
            input.push(0);
        }
        let text = self.run_text(&["mktree", "-z"], Some(&input))?;
        Ok(TreeId::parse(text.trim())?)
    }

    /// Writes a commit with explicit parents, in the order given.
    ///
    /// # Errors
    /// Refuses when Git rejects the tree or a parent.
    #[cfg(any(test, feature = "testing"))]
    pub fn write_commit(
        &self,
        tree: &TreeId,
        parents: &[&ObjectId],
        message: &str,
    ) -> Result<CommitId, GitError> {
        let mut args = vec!["commit-tree", tree.as_str()];
        for parent in parents {
            args.push("-p");
            args.push(parent.as_str());
        }
        args.extend(["-m", message]);
        let text = self.run_text(&args, None)?;
        Ok(CommitId::parse(text.trim())?)
    }

    /// Moves `reference` to `new` only if it still points at `expected`
    /// (`None`: only if it does not exist yet). The reference is hardened
    /// before this returns.
    ///
    /// # Errors
    /// [`GitError::RefMoved`] when the compare-and-swap loses.
    pub fn update_ref(
        &self,
        reference: &str,
        new: &ObjectId,
        expected: Option<&ObjectId>,
    ) -> Result<(), GitError> {
        let instruction = match expected {
            Some(old) => format!("update {reference} {new} {old}\n"),
            None => format!("create {reference} {new}\n"),
        };
        match self.run(&["update-ref", "--stdin"], Some(instruction.as_bytes())) {
            Ok(_) => Ok(()),
            Err(GitError::Failed { stderr, .. })
                if stderr.contains("but expected") || stderr.contains("already exists") =>
            {
                Err(GitError::RefMoved {
                    reference: reference.to_owned(),
                    expected: expected.map_or_else(|| "absent".into(), ToString::to_string),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// The tree of a commit.
    ///
    /// # Errors
    /// Refuses when the commit is missing.
    pub fn tree_of(&self, commit: &ObjectId) -> Result<TreeId, GitError> {
        let text = self.run_text(&["rev-parse", &format!("{commit}^{{tree}}")], None)?;
        Ok(TreeId::parse(text.trim())?)
    }

    /// The UTF-8 message of exactly this commit, including its original newlines.
    ///
    /// # Errors
    /// Refuses a missing or non-commit object, invalid UTF-8, or an unexpected
    /// Git answer. A formatted object identity distinguishes an empty message
    /// from a log command that found no commit.
    pub fn commit_message(&self, commit: &CommitId) -> Result<String, GitError> {
        let args = [
            "log",
            "-1",
            "--format=%H%x00%B",
            "--no-show-signature",
            "--end-of-options",
            commit.as_str(),
        ];
        let text = self.run_text(&args, None)?;
        let message = text
            .split_once('\0')
            .filter(|(id, _)| *id == commit.as_str())
            .and_then(|(_, message)| message.strip_suffix('\n'))
            .ok_or_else(|| GitError::Unexpected {
                args: args.join(" "),
                detail: "expected the requested commit and its message".into(),
            })?;
        Ok(message.to_owned())
    }

    /// Whether `ancestor` is reachable from `descendant` (a commit is its own ancestor).
    ///
    /// # Errors
    /// Refuses when Git fails for a reason other than "not an ancestor".
    pub fn is_ancestor(
        &self,
        ancestor: &ObjectId,
        descendant: &ObjectId,
    ) -> Result<bool, GitError> {
        let output = output_of(
            self.command(&[
                "merge-base",
                "--is-ancestor",
                ancestor.as_str(),
                descendant.as_str(),
            ])
            .stdin(Stdio::null()),
        )?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitError::Failed {
                args: format!("merge-base --is-ancestor {ancestor} {descendant}"),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }),
        }
    }

    /// The commits reachable from `rev` that changed `path`, newest first.
    ///
    /// # Errors
    /// Refuses when `rev` is missing or Git prints something that is not a commit.
    pub fn commits_touching(&self, rev: &str, path: &str) -> Result<Vec<CommitId>, GitError> {
        let text = self.run_text(
            &["log", "--format=%H", "--end-of-options", rev, "--", path],
            None,
        )?;
        text.lines()
            .map(|line| CommitId::parse(line).map_err(GitError::from))
            .collect()
    }

    /// A commit's committer time, in seconds since the Unix epoch.
    ///
    /// # Errors
    /// Refuses when the commit is missing.
    pub fn commit_time(&self, commit: &ObjectId) -> Result<u64, GitError> {
        let args = [
            "show",
            "-s",
            "--format=%ct",
            "--end-of-options",
            commit.as_str(),
        ];
        let text = self.run_text(&args, None)?;
        text.trim().parse().map_err(|_| GitError::Unexpected {
            args: args.join(" "),
            detail: text.trim().to_owned(),
        })
    }

    /// The value of `name` in the config Git reads for this repository, or
    /// `None` when it is unset.
    ///
    /// # Errors
    /// Refuses when Git fails for a reason other than an unset name.
    pub fn config(&self, name: &str) -> Result<Option<String>, GitError> {
        let output = self.output(&["config", "--get", name])?;
        match output.status.code() {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            )),
            Some(1) => Ok(None),
            _ => Err(GitError::Failed {
                args: format!("config --get {name}"),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            }),
        }
    }

    /// Whether an object exists in the repository.
    ///
    /// # Errors
    /// Refuses when Git cannot be run.
    pub fn has_object(&self, id: &ObjectId) -> Result<bool, GitError> {
        let status = start(
            self.command(&["cat-file", "-e", id.as_str()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )?
        .wait()
        .map_err(GitError::Io)?;
        Ok(status.success())
    }
}

/// Admit the complete size-framed response before returning any blob.
fn blob_answers(ids: &[ObjectId], mut bytes: &[u8]) -> Result<Vec<Vec<u8>>, GitError> {
    let mut answers = Vec::with_capacity(ids.len());
    for id in ids {
        let unexpected = |detail: &str| GitError::Unexpected {
            args: "cat-file --batch".into(),
            detail: format!("blob {id}: {detail}"),
        };
        let end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| unexpected("missing header"))?;
        let header =
            std::str::from_utf8(&bytes[..end]).map_err(|_| unexpected("non-text header"))?;
        let fields: Vec<_> = header.split(' ').collect();
        let [found, "blob", size] = fields.as_slice() else {
            return Err(unexpected("missing object or non-blob answer"));
        };
        if *found != id.as_str() {
            return Err(unexpected("response object differs from request"));
        }
        let size = size
            .parse::<usize>()
            .map_err(|_| unexpected("invalid byte length"))?;
        bytes = &bytes[end + 1..];
        let body = bytes
            .get(..size)
            .ok_or_else(|| unexpected("truncated body"))?;
        answers.push(body.to_vec());
        bytes = bytes[size..]
            .strip_prefix(b"\n")
            .ok_or_else(|| unexpected("missing body terminator"))?;
    }
    if !bytes.is_empty() {
        return Err(GitError::Unexpected {
            args: "cat-file --batch".into(),
            detail: format!(
                "unrequested answers after blob {}",
                ids.last().map_or("<none>", ObjectId::as_str)
            ),
        });
    }
    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    #[test]
    fn commit_messages_preserve_multiline_bodies_and_refuse_non_commits() {
        let (_dir, git) = testing::repo();
        git.run(&["config", "--local", "log.showSignature", "true"], None)
            .expect("signature display config must not change message reads");
        let parent = git
            .resolve("refs/heads/master")
            .expect("resolve")
            .expect("parent");
        let tree = git.tree_of(&parent).expect("tree");
        for message in ["title (issue-17)\n\nUTF-8 φ\n\n", ""] {
            let commit = git
                .write_commit(&tree, &[&parent], message)
                .expect("commit");
            let bytes = git
                .run(&["cat-file", "commit", commit.as_str()], None)
                .expect("raw commit");
            let raw = String::from_utf8(bytes).expect("UTF-8 fixture");
            let expected = raw.split_once("\n\n").expect("headers").1;
            assert_eq!(git.commit_message(&commit).expect("message"), expected);
        }
        let blob = git
            .write_blob(b"a blob is not a commit message")
            .expect("blob");
        assert!(
            git.commit_message(&CommitId::parse(blob.as_str()).expect("formatted id"))
                .is_err()
        );
        assert!(
            git.commit_message(&CommitId::parse(&"0".repeat(40)).expect("missing id"))
                .is_err()
        );
    }

    #[test]
    fn batch_blobs_keep_order_binary_bytes_and_empty_bodies() {
        let (_dir, git) = testing::repo();
        let binary = b"\0raw\xff\n\n";
        let first = git.write_blob(binary).expect("binary blob");
        let empty = git.write_blob(b"").expect("empty blob");
        let text = git.write_blob(b"last\n").expect("text blob");
        assert_eq!(
            git.read_blobs(&[text, first.clone(), empty, first])
                .expect("ordered blobs"),
            vec![
                b"last\n".to_vec(),
                binary.to_vec(),
                Vec::new(),
                binary.to_vec()
            ]
        );
        assert!(git.read_blobs(&[]).expect("empty request").is_empty());
    }

    #[test]
    fn batch_blobs_refuse_the_whole_population_on_missing_or_non_blob() {
        let (_dir, git) = testing::repo();
        let blob = git.write_blob(b"admitted\n").expect("blob");
        let tree = git.write_tree(&[]).expect("tree");
        let missing = ObjectId::parse(&"f".repeat(40)).expect("absent id");
        for invalid in [missing, tree.object().clone()] {
            let error = git
                .read_blobs(&[blob.clone(), invalid.clone(), blob.clone()])
                .expect_err("a valid prefix cannot admit the batch");
            assert!(error.to_string().contains(invalid.as_str()), "{error}");
        }
    }

    #[test]
    fn batch_blob_framing_refuses_every_unbound_answer() {
        let id = ObjectId::parse(&"a".repeat(40)).expect("id");
        let other = "b".repeat(40);
        let malformed = [
            Vec::new(),
            b"\xff\n".to_vec(),
            format!("{id} missing\n").into_bytes(),
            format!("{id} tree 1\nx\n").into_bytes(),
            format!("{other} blob 1\nx\n").into_bytes(),
            format!("{id} blob invalid\nx\n").into_bytes(),
            format!("{id} blob {}\nx\n", u128::MAX).into_bytes(),
            format!("{id} blob 3\nx\n").into_bytes(),
            format!("{id} blob 1\nx!").into_bytes(),
            format!("{id} blob 1\nx\nextra").into_bytes(),
        ];
        for bytes in malformed {
            let error = blob_answers(std::slice::from_ref(&id), &bytes)
                .expect_err("malformed response refuses");
            assert!(error.to_string().contains(id.as_str()), "{error}");
        }
        assert_eq!(
            blob_answers(
                std::slice::from_ref(&id),
                format!("{id} blob 1\nx\n").as_bytes()
            )
            .expect("exact response"),
            vec![b"x".to_vec()]
        );
    }

    #[test]
    fn batch_transport_handles_requests_and_answers_larger_than_a_pipe() {
        let (_dir, git) = testing::repo();
        let bytes = b"binary\0body\xffwith\nnewlines\n";
        let id = git.write_blob(bytes).expect("blob");
        let ids = vec![id; 4096];
        let bodies = git.read_blobs(&ids).expect("large duplex batch");
        assert_eq!(bodies.len(), ids.len());
        assert!(bodies.iter().all(|body| body == bytes));
    }

    #[test]
    fn blobs_trees_commits_and_refs_round_trip() {
        let (_dir, git) = testing::repo();
        let blob = git.write_blob(b"line\n").expect("blob");
        assert_eq!(git.read_blob(&blob).expect("read"), b"line\n");
        let entry = TreeEntry {
            mode: "100644".into(),
            kind: "blob".into(),
            id: blob,
            name: "note".into(),
        };
        let tree = git.write_tree(std::slice::from_ref(&entry)).expect("tree");
        assert_eq!(git.read_tree(tree.object()).expect("ls"), vec![entry]);
        let master = git
            .resolve("refs/heads/master")
            .expect("resolve")
            .expect("master");
        let commit = git
            .write_commit(&tree, &[&master], "event")
            .expect("commit");
        assert_eq!(git.tree_of(commit.object()).expect("tree of"), tree);
        assert!(git.resolve("refs/notes/x").expect("resolve").is_none());
        git.update_ref("refs/notes/x", commit.object(), None)
            .expect("create");
        assert_eq!(
            git.resolve("refs/notes/x").expect("resolve"),
            Some(commit.object().clone())
        );
    }

    #[test]
    fn a_lost_compare_and_swap_is_refused_as_moved() {
        let (_dir, git) = testing::repo();
        let master = git
            .resolve("refs/heads/master")
            .expect("ok")
            .expect("master");
        let tree = git.tree_of(&master).expect("tree");
        let next = git.write_commit(&tree, &[&master], "next").expect("commit");
        git.update_ref("refs/notes/y", &master, None)
            .expect("create");
        let stale = next.object().clone();
        let error = git
            .update_ref("refs/notes/y", next.object(), Some(&stale))
            .expect_err("stale expectation");
        assert!(matches!(error, GitError::RefMoved { .. }), "{error}");
        assert!(matches!(
            git.update_ref("refs/notes/y", next.object(), None),
            Err(GitError::RefMoved { .. })
        ));
    }
}
