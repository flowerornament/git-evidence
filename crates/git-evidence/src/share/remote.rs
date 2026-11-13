//! Sharing evidence between stores: `fetch` and `push`. What a push would
//! publish is [`crate::share::audit`]'s.
//!
//! ```text
//!    laptop  ──push──►  origin  ◄──push──  bench
//!            ◄─fetch──          ──fetch─►        combined by set union; nothing overwritten or lost
//! ```
//!
//! Git moves neither notes nor custom refs by default, and jj moves neither, so
//! evidence travels only through these verbs. `fetch` brings each kind's ref into
//! `refs/notes/remotes/<remote>/evidence/<kind>` and union-merges it into the
//! local ref with Git's own `cat_sort_uniq`: lines are unique and order-free, so
//! the merge is exact and never conflicts. `push` asks the remote what each
//! kind's ref holds and sends, audited, only what it lacks; a ref the remote
//! holds commits of that this store lacks means another store pushed first, so
//! it fetches, merges and pushes again. Nothing is ever forced: an update is a
//! fast-forward under a lease on what the remote held when asked. One lock per
//! store serialises the transfers of the workspaces sharing it; an audit runs
//! outside it.

use crate::format::id::{Kind, ObjectId};
use crate::rows::{TidyRows, cell, columns};
use crate::share::audit::{Outgoing, OwnNames, judge, notes_refs, own_names};
use crate::store::{NOTES, Store, StoreError, fetched_notes_prefix};
use git_process::GitError;
use serde::Serialize;
use std::collections::BTreeMap;

/// A push or fetch that could not run.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(
        "this store is a shallow clone: evidence history cannot be verified; fetch with --unshallow"
    )]
    Shallow,
    #[error(
        "{0:?} is not a configured remote: add it with `git remote add <name> <url>` and use its name"
    )]
    UnknownRemote(String),
    /// Pushing is off until the store says otherwise:
    /// pushed evidence is permanent in every clone.
    #[error(
        "pushing evidence is not enabled for this store: after a dry run (`git evidence push --dry-run`) and its audit, run `git config --local evidence.push true`"
    )]
    PushNotEnabled,
    /// Nothing leaves while the audit has findings: pushed evidence is
    /// permanent in every clone, so every push is an audited push.
    #[error(
        "the audit has {0} finding(s), so nothing was pushed: run `git evidence audit`; a push sends the notes' whole history, so a finding is fixed by rewriting that history, not by removing the note"
    )]
    AuditFindings(usize),
    /// The audit keeps this machine's names out of pushed evidence, so it
    /// cannot run without them.
    #[error(
        "cannot read this machine's name ({command}: {detail}), so the audit cannot keep it out of pushed evidence: make `{command}` work, then audit again"
    )]
    HostName { command: String, detail: String },
    #[error("git {verb} with {remote} failed: {stderr}")]
    Transport {
        verb: &'static str,
        remote: String,
        stderr: String,
    },
}

/// What happened to one ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    Created,
    Updated,
    UpToDate,
    /// Git rejected the update, naming why.
    Refused(String),
}

/// One ref's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefUpdate {
    pub reference: String,
    pub change: UpdateKind,
}

/// What a push or fetch did, ref by ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transfer {
    pub remote: String,
    pub refs: Vec<RefUpdate>,
    /// Kinds whose local ref took in the remote's events.
    pub merged: Vec<String>,
}

impl Transfer {
    /// Whether any ref was refused.
    #[must_use]
    pub fn refused(&self) -> bool {
        self.refs
            .iter()
            .any(|r| matches!(r.change, UpdateKind::Refused(_)))
    }
}

/// The remote evidence travels through: Git config `evidence.remote`, else `origin`.
///
/// # Errors
/// Fails when Git cannot be run.
pub(crate) fn default_remote(store: &Store) -> Result<String, RemoteError> {
    let configured = store
        .git()
        .output(&["config", "--get", "evidence.remote"])?;
    let name = String::from_utf8_lossy(&configured.stdout)
        .trim()
        .to_owned();
    Ok(if configured.status.success() && !name.is_empty() {
        name
    } else {
        "origin".to_owned()
    })
}

/// Refuses an incomplete store and a remote that is not a configured name:
/// fetched copies are filed under the remote's name, never under a URL or path.
fn check(store: &Store, remote: &str) -> Result<(), RemoteError> {
    store.require_complete()?;
    let named = !remote.is_empty()
        && !remote.contains(['/', ':', '\\'])
        && store
            .git()
            .output(&["config", "--get", &format!("remote.{remote}.url")])?
            .status
            .success();
    if named {
        Ok(())
    } else {
        Err(RemoteError::UnknownRemote(remote.to_owned()))
    }
}

/// Fetches every kind's ref from `remote` and union-merges each into the local ref.
///
/// # Errors
/// Refuses an incomplete store, an unknown remote, and a transport failure; a
/// rejected ref is not an error but a [`UpdateKind::Refused`] in the transfer.
pub(crate) fn fetch(store: &Store, remote: &str) -> Result<Transfer, RemoteError> {
    check(store, remote)?;
    let _lock = store.lock("remote")?;
    fetch_and_merge(store, remote)
}

fn fetch_and_merge(store: &Store, remote: &str) -> Result<Transfer, RemoteError> {
    let prefix = fetched_notes_prefix(remote);
    let spec = format!("{NOTES}*:{prefix}*");
    let output = store.git().transport(&[
        "fetch",
        "--porcelain",
        "--no-write-fetch-head",
        remote,
        &spec,
    ])?;
    let refs = parse_fetch(&String::from_utf8_lossy(&output.stdout));
    if !output.status.success()
        && !refs
            .iter()
            .any(|c| matches!(c.change, UpdateKind::Refused(_)))
    {
        return Err(RemoteError::Transport {
            verb: "fetch",
            remote: remote.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let fetched = store
        .git()
        .run(&["for-each-ref", "--format=%(refname)", &prefix], None)?;
    let mut merged = Vec::new();
    for name in String::from_utf8_lossy(&fetched).lines() {
        let Some(kind) = name.strip_prefix(&prefix).and_then(|k| Kind::parse(k).ok()) else {
            continue;
        };
        let local = Store::notes_ref(&kind);
        let _kind_lock = store.lock(&local)?;
        let theirs = store.git().resolve(name)?;
        let ours = store.git().resolve(&local)?;
        match (ours, theirs) {
            (_, None) => {}
            (None, Some(theirs)) => {
                store.git().update_ref(&local, &theirs, None)?;
                merged.push(kind.to_string());
            }
            (Some(ours), Some(theirs)) if ours == theirs => {}
            (Some(_), Some(_)) => {
                store.git().run(
                    &[
                        "notes",
                        "--ref",
                        &local,
                        "merge",
                        "-q",
                        "-s",
                        "cat_sort_uniq",
                        name,
                    ],
                    None,
                )?;
                merged.push(kind.to_string());
            }
        }
    }
    Ok(Transfer {
        remote: remote.to_owned(),
        refs,
        merged,
    })
}

/// Pushes every kind's ref to `remote`. A ref the remote holds commits of
/// that this store has not merged, or a push refused because the remote
/// moved, is fetched, merged and pushed again, up to three times in all.
///
/// # Errors
/// As [`fetch`].
pub(crate) fn push(store: &Store, remote: &str) -> Result<Transfer, RemoteError> {
    // A store that does not push says so before any name is looked up.
    require_push_enabled(store)?;
    push_against(store, remote, &own_names()?)
}

fn require_push_enabled(store: &Store) -> Result<(), RemoteError> {
    let enabled = store
        .git()
        .output(&["config", "--local", "--bool", "--get", "evidence.push"])?;
    if String::from_utf8_lossy(&enabled.stdout).trim() == "true" {
        Ok(())
    } else {
        Err(RemoteError::PushNotEnabled)
    }
}

/// How many times a push asks the remote, merges what it holds, and sends.
const ATTEMPTS: usize = 3;

/// [`push`], audited against `own` instead of this user's and host's names.
///
/// A push costs what changed since the remote last took this store's
/// evidence. It asks the remote what each ref holds; a ref the remote holds
/// already is neither judged nor sent; any other is judged over the note
/// versions the remote does not hold ([`judge`]) and sent only when that
/// judgement is clean, under a lease on what the remote held when asked:
/// were the remote to move in between, Git refuses the ref rather than send
/// history judged against a different remote. The store's remote lock is
/// held only while sending and merging, never while judging.
pub(crate) fn push_against(
    store: &Store,
    remote: &str,
    own: &OwnNames,
) -> Result<Transfer, RemoteError> {
    require_push_enabled(store)?;
    check(store, remote)?;
    let mut merged = Vec::new();
    for attempt in 1..=ATTEMPTS {
        let plan = Plan::of(store, &notes_refs(store)?, &remote_refs(store, remote)?)?;
        if !plan.unmerged.is_empty() && attempt < ATTEMPTS {
            let _lock = store.lock("remote")?;
            merged.extend(fetch_and_merge(store, remote)?.merged);
            continue;
        }
        let mut refs: Vec<RefUpdate> = plan
            .held
            .iter()
            .map(|reference| RefUpdate {
                reference: reference.clone(),
                change: UpdateKind::UpToDate,
            })
            .chain(plan.unmerged.iter().map(|reference| RefUpdate {
                reference: reference.clone(),
                change: UpdateKind::Refused(UNMERGED.to_owned()),
            }))
            .collect();
        if plan.outgoing.is_empty() {
            return Ok(transfer(remote, refs, merged));
        }
        let findings = judge(store, own, &plan.outgoing)?.findings.len();
        if findings > 0 {
            return Err(RemoteError::AuditFindings(findings));
        }
        let _lock = store.lock("remote")?;
        // Each ref is pushed by the commit judged, whatever is recorded
        // meanwhile, and only if the remote still holds what it held when
        // asked: what is judged is exactly what is sent.
        let leases: Vec<String> = plan
            .outgoing
            .iter()
            .map(|out| {
                format!(
                    "--force-with-lease={}:{}",
                    out.name,
                    out.held.as_deref().unwrap_or_default()
                )
            })
            .collect();
        let specs: Vec<String> = plan
            .outgoing
            .iter()
            .map(|out| format!("{}:{}", out.commit, out.name))
            .collect();
        // Evidence refs are not code: the store's pre-push hooks (bd, LFS)
        // must neither run on them nor refuse them.
        let mut args = vec!["push", "--porcelain", "--no-verify"];
        args.extend(leases.iter().map(String::as_str));
        args.push(remote);
        args.extend(specs.iter().map(String::as_str));
        let output = store.git().transport(&args)?;
        let pushed = parse_push(&String::from_utf8_lossy(&output.stdout));
        let refused = pushed
            .iter()
            .any(|c| matches!(c.change, UpdateKind::Refused(_)));
        if !output.status.success() && !refused {
            return Err(RemoteError::Transport {
                verb: "push",
                remote: remote.to_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        refs.extend(pushed);
        if !refused || attempt == ATTEMPTS {
            return Ok(transfer(remote, refs, merged));
        }
        merged.extend(fetch_and_merge(store, remote)?.merged);
    }
    unreachable!("the loop returns on its last attempt")
}

/// Why a ref the remote holds unmerged commits of is not pushed on the last
/// attempt.
const UNMERGED: &str =
    "the remote holds evidence this store has not merged; fetched and merged, push again";

/// A transfer's rows, ordered by reference.
fn transfer(remote: &str, mut refs: Vec<RefUpdate>, merged: Vec<String>) -> Transfer {
    refs.sort_by(|a, b| a.reference.cmp(&b.reference));
    Transfer {
        remote: remote.to_owned(),
        refs,
        merged,
    }
}

/// What one push attempt does with each local ref, given what the remote
/// holds.
struct Plan {
    /// Refs the remote holds at the local commit: nothing to send.
    held: Vec<String>,
    /// Refs the remote lacks, or holds at an ancestor of the local commit.
    outgoing: Vec<Outgoing>,
    /// Refs the remote holds at a commit the local one does not contain:
    /// merged before they are sent.
    unmerged: Vec<String>,
}

impl Plan {
    fn of(
        store: &Store,
        local: &[(String, String)],
        theirs: &BTreeMap<String, String>,
    ) -> Result<Self, RemoteError> {
        let mut plan = Self {
            held: Vec::new(),
            outgoing: Vec::new(),
            unmerged: Vec::new(),
        };
        for (name, commit) in local {
            match theirs.get(name) {
                None => plan.outgoing.push(Outgoing {
                    name: name.clone(),
                    commit: commit.clone(),
                    held: None,
                }),
                Some(tip) if tip == commit => plan.held.push(name.clone()),
                Some(tip) => {
                    let tip_id = ObjectId::parse(tip).map_err(GitError::from)?;
                    let ours = ObjectId::parse(commit).map_err(GitError::from)?;
                    if store.git().has_object(&tip_id)?
                        && store.git().is_ancestor(&tip_id, &ours)?
                    {
                        plan.outgoing.push(Outgoing {
                            name: name.clone(),
                            commit: commit.clone(),
                            held: Some(tip.clone()),
                        });
                    } else {
                        plan.unmerged.push(name.clone());
                    }
                }
            }
        }
        Ok(plan)
    }
}

/// Each evidence ref `remote` holds now and the commit it names, as the
/// remote itself says.
fn remote_refs(store: &Store, remote: &str) -> Result<BTreeMap<String, String>, RemoteError> {
    let pattern = format!("{NOTES}*");
    let output = store
        .git()
        .transport(&["ls-remote", "--refs", remote, &pattern])?;
    if !output.status.success() {
        return Err(RemoteError::Transport {
            verb: "ls-remote",
            remote: remote.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(_, name)| name.starts_with(NOTES))
        .map(|(commit, name)| (name.to_owned(), commit.to_owned()))
        .collect())
}

/// What [`push`] would send, without sending it: the refs and how each would
/// change. Allowed whether or not pushing is enabled, so the first push can be
/// seen before it is approved.
///
/// # Errors
/// As [`push`], except that pushing need not be enabled.
pub(crate) fn push_dry_run(store: &Store, remote: &str) -> Result<Transfer, RemoteError> {
    check(store, remote)?;
    let _lock = store.lock("remote")?;
    let specs: Vec<String> = notes_refs(store)?
        .iter()
        .map(|(name, commit)| format!("{commit}:{name}"))
        .collect();
    if specs.is_empty() {
        return Ok(transfer(remote, Vec::new(), Vec::new()));
    }
    // Hooks never run on evidence refs, in a dry run least of all.
    let mut args = vec!["push", "--porcelain", "--no-verify", "--dry-run", remote];
    args.extend(specs.iter().map(String::as_str));
    let output = store.git().transport(&args)?;
    let refs = parse_push(&String::from_utf8_lossy(&output.stdout));
    let refused = refs
        .iter()
        .any(|c| matches!(c.change, UpdateKind::Refused(_)));
    if !output.status.success() && !refused {
        return Err(RemoteError::Transport {
            verb: "push",
            remote: remote.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    // A dry run reports a refusal as it is: merging would change local refs.
    Ok(transfer(remote, refs, Vec::new()))
}

/// `git fetch --porcelain`: `<flag> <old> <new> <local ref>` per updated ref.
fn parse_fetch(text: &str) -> Vec<RefUpdate> {
    text.lines()
        .filter_map(|line| {
            let flag = line.chars().next()?;
            let reference = line.rsplit(' ').next()?.to_owned();
            let change = match flag {
                '*' => UpdateKind::Created,
                ' ' | '+' => UpdateKind::Updated,
                '=' => UpdateKind::UpToDate,
                // Fetch's porcelain format carries no reason; never guess one.
                '!' => UpdateKind::Refused("rejected by git fetch".into()),
                _ => return None,
            };
            Some(RefUpdate { reference, change })
        })
        .collect()
}

/// `git push --porcelain`: `<flag>\t<from>:<to>\t<summary>` per ref.
fn parse_push(text: &str) -> Vec<RefUpdate> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let flag = fields.next()?;
            let refs = fields.next()?;
            let summary = fields.next().unwrap_or_default();
            let reference = refs.split_once(':').map_or(refs, |(_, to)| to).to_owned();
            let change = match flag {
                "*" => UpdateKind::Created,
                " " | "+" => UpdateKind::Updated,
                "=" => UpdateKind::UpToDate,
                "!" => UpdateKind::Refused(summary.trim().to_owned()),
                _ => return None,
            };
            Some(RefUpdate { reference, change })
        })
        .collect()
}

/// A fetch or push: one row per ref and what happened to it (`created`,
/// `updated`, `up_to_date`, `refused` with Git's reason in `detail`), then one
/// row per kind merged from the remote (`merged`).
impl TidyRows for Transfer {
    fn header(&self) -> Vec<String> {
        columns(&["remote", "reference", "change", "detail"])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        let mut rows: Vec<Vec<Option<String>>> = self
            .refs
            .iter()
            .map(|r| {
                let (change, detail) = match &r.change {
                    UpdateKind::Created => ("created", None),
                    UpdateKind::Updated => ("updated", None),
                    UpdateKind::UpToDate => ("up_to_date", None),
                    UpdateKind::Refused(why) => ("refused", cell(why)),
                };
                vec![cell(&self.remote), cell(&r.reference), cell(change), detail]
            })
            .collect();
        rows.extend(self.merged.iter().map(|kind| {
            vec![
                cell(&self.remote),
                cell(format!("{}{kind}", crate::store::NOTES)),
                cell("merged"),
                None,
            ]
        }));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_lines_parse_into_changes() {
        let push = "To /r\n*\trefs/notes/evidence/a/p:refs/notes/evidence/a/p\t[new reference]\n\
                    !\trefs/notes/evidence/b/p:refs/notes/evidence/b/p\t[rejected] (non-fast-forward)\n\
                    =\trefs/notes/evidence/c/p:refs/notes/evidence/c/p\t[up to date]\nDone\n";
        let changes = parse_push(push);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].change, UpdateKind::Created);
        assert_eq!(
            changes[1].change,
            UpdateKind::Refused("[rejected] (non-fast-forward)".into())
        );
        let fetch = "* 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/notes/remotes/o/evidence/a/p\n\
                     ! 2222222222222222222222222222222222222222 3333333333333333333333333333333333333333 refs/notes/remotes/o/evidence/b/p\n";
        let changes = parse_fetch(fetch);
        assert_eq!(changes[0].reference, "refs/notes/remotes/o/evidence/a/p");
        assert!(matches!(changes[1].change, UpdateKind::Refused(_)));
    }
}
