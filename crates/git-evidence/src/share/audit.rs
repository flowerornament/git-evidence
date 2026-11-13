//! What a push would publish, and what in it must not be published: the notes
//! each kind's ref holds, what does not fold, and anything that looks
//! personal. `git evidence audit` judges every note in each ref's history;
//! a push judges what it sends, the note versions the remote does not hold
//! yet ([`Outgoing`]), and sends nothing while that has a finding
//! ([`crate::share::remote`]).
//!
//! ```text
//!    refs/notes/evidence/<kind> ──► note versions sent ──► Audit
//!          (^ what the remote holds)    + the version the     refs        notes, lines, bytes per kind
//!                                         remote holds of     identities  who committed the notes sent
//!                                         each tree touched   findings    personal text · lines that do not fold
//! ```
//!
//! A push sends history, so a note removed from the tree is still judged.
//! "Personal" is judged against this user's and this machine's own names, which
//! a test replaces with chosen ones ([`OwnNames`]).

use crate::format::event::{Event, EventLine};
use crate::format::fold::{Problem, fold};
use crate::format::id::{Kind, Machine, ObjectId, RunId, TreeId};
use crate::rows::{TidyRows, cell, columns};
use crate::share::remote::RemoteError;
use crate::store::{NOTES, Store, StoreError};
use git_process::GitError;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// What would be pushed, inspected before the first push.
/// Every finding names where it is; an empty `findings` is a clean store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audit {
    pub refs: Vec<RefAudit>,
    /// Every author and committer of the notes commits, which are pushed too:
    /// look them over before the first push.
    pub identities: Vec<String>,
    pub findings: Vec<Finding>,
}

/// One kind's ref: how many trees carry notes, and their encoded size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefAudit {
    pub name: String,
    pub notes: usize,
    pub lines: usize,
    pub bytes: u64,
}

/// Something that must not be pushed, or must be looked at first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A line or run the fold cannot read.
    Violation {
        kind: Kind,
        tree: TreeId,
        problem: Problem,
    },
    /// Text that looks personal: a home or temporary path, a `.local` host,
    /// an address, this user's own login or home. `run` and `seq` are absent
    /// when the line does not name them.
    Personal {
        kind: Kind,
        tree: TreeId,
        run: Option<RunId>,
        seq: Option<u32>,
        sample: String,
    },
}

/// One ref as a push sends it: the commit sent, and the commit the remote
/// already holds for that ref, if any, whose history is not sent again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub name: String,
    pub commit: String,
    pub held: Option<String>,
}

/// Who committed what is judged, and what in it must not be published.
pub(crate) struct Judgement {
    pub identities: Vec<String>,
    pub findings: Vec<Finding>,
}

/// Reads every kind's notes, and who committed them, as they would be pushed;
/// reports their size, what does not fold, and anything that looks personal.
///
/// # Errors
/// A Git failure.
pub(crate) fn audit(store: &Store) -> Result<Audit, RemoteError> {
    audit_against(store, &own_names()?)
}

/// [`audit`] against `own`, the lower-case names that must not appear: this
/// user's and this host's in production, chosen ones in a test, so the audit's
/// behaviour does not depend on the machine it runs on. It judges each ref's
/// whole history, as a push to a remote that holds none of it would send.
pub(crate) fn audit_against(store: &Store, own: &OwnNames) -> Result<Audit, RemoteError> {
    let refs = notes_refs(store)?;
    let whole: Vec<Outgoing> = refs
        .iter()
        .map(|(name, commit)| Outgoing {
            name: name.clone(),
            commit: commit.clone(),
            held: None,
        })
        .collect();
    let judgement = judge(store, own, &whole)?;
    let refs = refs
        .iter()
        .map(|(name, commit)| size_now(store, name, commit))
        .collect::<Result<_, _>>()?;
    Ok(Audit {
        refs,
        identities: judgement.identities,
        findings: judgement.findings,
    })
}

/// Every kind's notes ref and the commit it names now.
pub(crate) fn notes_refs(store: &Store) -> Result<Vec<(String, String)>, RemoteError> {
    let listing = store.git().run(
        &["for-each-ref", "--format=%(refname) %(objectname)", NOTES],
        None,
    )?;
    Ok(String::from_utf8_lossy(&listing)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, commit)| (name.to_owned(), commit.to_owned()))
        .collect())
}

/// Blobs read from Git in one batch: enough to keep the number of `git`
/// processes small, few enough that a batch's bytes stay modest.
const BATCH: usize = 256;

/// The judgement of `refs` as a push sends them: every note version in each
/// ref's history that the remote does not hold yet, read together with the
/// version the remote holds of the same tree, so that a line sent is judged
/// against the lines already published beside it. Each tree's versions are
/// read as one note of their distinct lines, so a line repeated by appends is
/// judged once and distinct lines are never merged.
///
/// Host names the store publishes itself (its declared machine, its kinds,
/// machines its open lines name) are not personal. They are gathered from
/// everything read before any line is judged, so ref order never decides; a
/// push reads less than the whole history, so it can only know fewer of them
/// and refuse more, never pass more.
///
/// A ref whose kind, or a note whose tree, is not a valid name is refused:
/// the audit cannot say what it holds, and a push would send it.
pub(crate) fn judge(
    store: &Store,
    own: &OwnNames,
    refs: &[Outgoing],
) -> Result<Judgement, RemoteError> {
    // A shallow or partial clone hides history and blobs a push would send,
    // so a clean audit of it would be a partial answer passed off as complete.
    store.require_complete()?;
    let mut public = BTreeSet::new();
    match store.machine() {
        Ok(machine) => {
            public.insert(machine.as_str().to_ascii_lowercase());
        }
        Err(StoreError::NoMachine) => {}
        Err(cause) => return Err(cause.into()),
    }
    let every_name = own.not_public(&BTreeSet::new());
    let mut identities = BTreeSet::new();
    let mut judged = Vec::new();
    for outgoing in refs {
        let Some(kind) = outgoing.name.strip_prefix(NOTES) else {
            continue;
        };
        let kind = Kind::parse(kind).map_err(GitError::from)?;
        public.insert(kind.as_str().to_ascii_lowercase());
        identities.extend(committers(store, outgoing)?);
        let trees: Vec<_> = versions(store, outgoing)?.into_iter().collect();
        judged.extend(judge_versions(
            store,
            &kind,
            &trees,
            &every_name,
            &mut public,
        )?);
    }
    // A line flagged against the names that are not public is flagged against
    // every name, so only the lines kept above need judging again.
    let own = own.not_public(&public);
    let findings = judged
        .into_iter()
        .flat_map(|tree| tree.findings(&own))
        .collect();
    Ok(Judgement {
        identities: identities.into_iter().collect(),
        findings,
    })
}

/// Judges each tree's note versions, read in batches; machines the lines
/// declare are added to `public`. Only trees with something to report are
/// kept.
fn judge_versions(
    store: &Store,
    kind: &Kind,
    trees: &[(TreeId, BTreeSet<ObjectId>)],
    every_name: &[String],
    public: &mut BTreeSet<String>,
) -> Result<Vec<TreeJudged>, RemoteError> {
    let mut judged = Vec::new();
    for batch in batches(trees) {
        let ids: Vec<ObjectId> = batch
            .iter()
            .flat_map(|(_, blobs)| blobs.iter().cloned())
            .collect();
        let mut bodies = store.git().read_blobs(&ids)?.into_iter();
        for (tree, blobs) in batch {
            let notes: Vec<Vec<u8>> = bodies.by_ref().take(blobs.len()).collect();
            let lines = distinct_lines(&notes);
            public.extend(lines.iter().filter_map(|line| machine_declared(line)));
            let tree_judged = judge_tree(kind, tree, &lines, every_name);
            if !tree_judged.is_clean() {
                judged.push(tree_judged);
            }
        }
    }
    Ok(judged)
}

/// Consecutive runs of `trees` whose versions together make about one
/// [`BATCH`] of blobs; a tree's versions are never split.
fn batches(
    trees: &[(TreeId, BTreeSet<ObjectId>)],
) -> impl Iterator<Item = &[(TreeId, BTreeSet<ObjectId>)]> {
    let mut rest = trees;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut blobs = 0;
        let mut end = 0;
        while end < rest.len() && (end == 0 || blobs + rest[end].1.len() <= BATCH) {
            blobs += rest[end].1.len();
            end += 1;
        }
        let (batch, after) = rest.split_at(end);
        rest = after;
        Some(batch)
    })
}

/// Every author and committer of the notes commits `outgoing` sends.
fn committers(store: &Store, outgoing: &Outgoing) -> Result<Vec<String>, RemoteError> {
    let held = outgoing.held.as_ref().map(|held| format!("^{held}"));
    let mut args = vec!["log", "--format=%an <%ae>%n%cn <%ce>", &outgoing.commit];
    args.extend(held.as_deref());
    let people = store.git().run(&args, None)?;
    Ok(String::from_utf8_lossy(&people)
        .lines()
        .map(str::to_owned)
        .collect())
}

/// The note versions `outgoing` sends, by the tree each is about, and for
/// each tree touched, the version the remote already holds.
fn versions(
    store: &Store,
    outgoing: &Outgoing,
) -> Result<BTreeMap<TreeId, BTreeSet<ObjectId>>, RemoteError> {
    let held = outgoing.held.as_ref().map(|held| format!("^{held}"));
    let mut args = vec![
        "rev-list",
        "--objects",
        "--filter=object:type=blob",
        &outgoing.commit,
    ];
    args.extend(held.as_deref());
    let mut by_tree: BTreeMap<TreeId, BTreeSet<ObjectId>> = BTreeMap::new();
    let sent = store.git().run(&args, None)?;
    for line in String::from_utf8_lossy(&sent).lines() {
        // `<blob> <path>`; a commit's line has no path.
        if let Some((blob, path)) = line.split_once(' ') {
            by_tree
                .entry(tree_of(path)?)
                .or_default()
                .insert(ObjectId::parse(blob).map_err(GitError::from)?);
        }
    }
    if let Some(held) = &outgoing.held
        && !by_tree.is_empty()
    {
        for (tree, blob, _) in notes_at(store, held)? {
            if let Some(versions) = by_tree.get_mut(&tree) {
                versions.insert(blob);
            }
        }
    }
    Ok(by_tree)
}

/// The tree a note is about, from its path less fan-out slashes.
fn tree_of(path: &str) -> Result<TreeId, GitError> {
    TreeId::parse(&path.replace('/', "")).map_err(GitError::from)
}

/// Each note the notes commit `commit` holds: its tree, blob and size.
fn notes_at(store: &Store, commit: &str) -> Result<Vec<(TreeId, ObjectId, u64)>, RemoteError> {
    let listing = store.git().run(&["ls-tree", "-r", "-l", commit], None)?;
    let mut notes = Vec::new();
    for line in String::from_utf8_lossy(&listing).lines() {
        // `<mode> blob <blob> <size>\t<path>`, the size padded with spaces.
        let unexpected = || GitError::Unexpected {
            args: format!("ls-tree -r -l {commit}"),
            detail: line.to_owned(),
        };
        let (meta, path) = line.split_once('\t').ok_or_else(unexpected)?;
        let mut fields = meta.split_whitespace().skip(2);
        let (Some(blob), Some(size)) = (fields.next(), fields.next()) else {
            return Err(unexpected().into());
        };
        let size = size.parse().map_err(|_| unexpected())?;
        notes.push((
            tree_of(path)?,
            ObjectId::parse(blob).map_err(GitError::from)?,
            size,
        ));
    }
    Ok(notes)
}

/// The size of the notes the ref `name` holds at `commit`.
fn size_now(store: &Store, name: &str, commit: &str) -> Result<RefAudit, RemoteError> {
    let notes = notes_at(store, commit)?;
    let mut lines = 0;
    for batch in notes.chunks(BATCH) {
        let ids: Vec<ObjectId> = batch.iter().map(|(_, blob, _)| blob.clone()).collect();
        for note in store.git().read_blobs(&ids)? {
            lines += note
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .count();
        }
    }
    Ok(RefAudit {
        name: name.to_owned(),
        notes: notes.len(),
        lines,
        bytes: notes.iter().map(|(_, _, size)| size).sum(),
    })
}

/// The distinct non-empty lines of a tree's note versions, in the order first
/// seen.
fn distinct_lines(notes: &[Vec<u8>]) -> Vec<&[u8]> {
    let mut seen = HashSet::new();
    notes
        .iter()
        .flat_map(|note| note.split(|b| *b == b'\n'))
        .filter(|line| !line.is_empty() && seen.insert(*line))
        .collect()
}

/// The machine an open line names, lower case, when it is a valid name.
fn machine_declared(line: &[u8]) -> Option<String> {
    // Only an open line names a machine; parse no other.
    if !line.windows(6).any(|w| w == b"\"open\"") {
        return None;
    }
    let EventLine {
        event: Event::Open(open),
        ..
    } = serde_json::from_slice(line).ok()?
    else {
        return None;
    };
    Machine::parse(&open.environment.machine)
        .ok()
        .map(|machine| machine.as_str().to_ascii_lowercase())
}

/// One tree's judgement before the public names are known: the runs that do
/// not fold, and the lines that look personal against every own name.
struct TreeJudged {
    kind: Kind,
    tree: TreeId,
    violations: Vec<Finding>,
    suspect: Vec<Vec<u8>>,
}

impl TreeJudged {
    fn is_clean(&self) -> bool {
        self.violations.is_empty() && self.suspect.is_empty()
    }

    /// Its findings once `own` is the names that are not public.
    fn findings(self, own: &[String]) -> Vec<Finding> {
        let mut findings = self.violations;
        for raw in &self.suspect {
            let text = String::from_utf8_lossy(raw);
            if let Some(sample) = personal(&text, own) {
                let parsed = serde_json::from_slice::<serde_json::Value>(raw).ok();
                findings.push(Finding::Personal {
                    kind: self.kind.clone(),
                    tree: self.tree.clone(),
                    run: parsed
                        .as_ref()
                        .and_then(|v| v.get("run")?.as_str())
                        .and_then(|run| run.parse().ok()),
                    seq: parsed
                        .as_ref()
                        .and_then(|v| v.get("seq")?.as_u64())
                        .and_then(|s| u32::try_from(s).ok()),
                    sample,
                });
            }
        }
        findings
    }
}

/// What one tree's distinct lines hold that must not be pushed: runs that do
/// not fold, and text that looks personal against `every_name`.
fn judge_tree(kind: &Kind, tree: &TreeId, lines: &[&[u8]], every_name: &[String]) -> TreeJudged {
    let violations = fold(&lines.join(&b'\n'))
        .violations
        .into_iter()
        .map(|violation| Finding::Violation {
            kind: kind.clone(),
            tree: tree.clone(),
            problem: violation.problem,
        })
        .collect();
    let suspect = lines
        .iter()
        .filter(|raw| personal(&String::from_utf8_lossy(raw), every_name).is_some())
        .map(|raw| raw.to_vec())
        .collect();
    TreeJudged {
        kind: kind.clone(),
        tree: tree.clone(),
        violations,
        suspect,
    }
}

/// This user's and this machine's own names, which a push's audit refuses to
/// publish. Person names always stay private; host names may already be
/// public roles.
#[derive(Debug)]
pub struct OwnNames {
    person: Vec<String>,
    host: Vec<String>,
}

impl OwnNames {
    /// Chosen names (any case), for a judgement that holds on every machine.
    #[must_use]
    pub fn new(person: Vec<String>, host: Vec<String>) -> Self {
        Self {
            person: normalized_names(person),
            host: normalized_names(host),
        }
    }

    /// The names a push's audit reads on this machine: the login and home,
    /// and every name the system gives the machine.
    ///
    /// # Errors
    /// [`RemoteError::HostName`] when a machine-name lookup fails.
    pub fn of_this_machine() -> Result<Self, RemoteError> {
        own_names()
    }

    /// Whether `text` can never be what blocks a push: neither it nor its
    /// JSON string form looks personal (a home or temporary path, a `.local`
    /// host, an address, or one of these names as a word, host names
    /// included even when the store already publishes them). A producer
    /// asks this of text that comes from outside it before recording it.
    #[must_use]
    pub fn publishable(&self, text: &str) -> bool {
        let every: Vec<String> = self.person.iter().chain(&self.host).cloned().collect();
        let quoted = serde_json::Value::String(text.to_owned()).to_string();
        personal(text, &every).is_none() && personal(&quoted, &every).is_none()
    }

    /// The names that must not appear: every person name, and each host
    /// name the store does not already publish.
    fn not_public(&self, public: &BTreeSet<String>) -> Vec<String> {
        self.host
            .iter()
            .filter(|name| !public.contains(&name.trim().to_ascii_lowercase()))
            .chain(self.person.iter())
            .cloned()
            .collect()
    }
}

pub(crate) fn own_names() -> Result<OwnNames, RemoteError> {
    let person = ["USER", "LOGNAME", "HOME"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect();
    // uname is in every POSIX base system; `hostname` is a package of its own
    // on Debian.
    let mut asked = vec![Lookup::required(&["uname", "-n"])];
    if cfg!(target_os = "macos") {
        // macOS ships scutil at this path; launchd's PATH does not reach
        // /usr/sbin, and a lookup that cannot run refuses the push.
        asked.push(Lookup::may_be_unset(&[
            "/usr/sbin/scutil",
            "--get",
            "LocalHostName",
        ]));
        asked.push(Lookup::may_be_unset(&[
            "/usr/sbin/scutil",
            "--get",
            "ComputerName",
        ]));
    }
    let host = std::env::var("HOSTNAME")
        .ok()
        .into_iter()
        .chain(host_names(&asked)?)
        .collect();
    Ok(OwnNames::new(person, host))
}

fn normalized_names(names: Vec<String>) -> Vec<String> {
    let mut names: Vec<_> = names
        .into_iter()
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| name.len() >= 3 && name != "/")
        .collect();
    names.sort();
    names.dedup();
    names
}

/// One way the system reports this machine's name.
struct Lookup<'a> {
    argv: &'a [&'a str],
    /// The system may report the name as not set (`scutil`: exit 1, "not set").
    may_be_unset: bool,
}

impl<'a> Lookup<'a> {
    const fn required(argv: &'a [&'a str]) -> Self {
        Self {
            argv,
            may_be_unset: false,
        }
    }

    const fn may_be_unset(argv: &'a [&'a str]) -> Self {
        Self {
            argv,
            may_be_unset: true,
        }
    }
}

/// This machine's names, as the system reports them, and each one's first
/// label (`kestrel` of `kestrel.local`): a role such as `laptop` names a
/// machine in evidence, its host name never does.
///
/// # Errors
/// [`RemoteError::HostName`] when a lookup cannot start, fails, or gives no
/// name, unless the system says that name is not set: an audit that knew
/// fewer of this machine's names would pass what it should refuse.
fn host_names(asked: &[Lookup<'_>]) -> Result<Vec<String>, RemoteError> {
    let mut names = Vec::new();
    for lookup in asked {
        let command = lookup.argv.join(" ");
        let unreadable = |detail: String| RemoteError::HostName {
            command: command.clone(),
            detail,
        };
        let output = git_process::output_of(
            std::process::Command::new(lookup.argv[0])
                .args(&lookup.argv[1..])
                .stdin(std::process::Stdio::null()),
        )
        .map_err(|error| {
            unreadable(match error {
                git_process::GitError::Spawn(cause) | git_process::GitError::Io(cause) => {
                    cause.to_string()
                }
                other => other.to_string(),
            })
        })?;
        let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() && !name.is_empty() {
            if let Some((label, _)) = name.split_once('.') {
                names.push(label.to_owned());
            }
            names.push(name);
            continue;
        }
        let said = format!("{name} {}", String::from_utf8_lossy(&output.stderr).trim());
        if lookup.may_be_unset && output.status.code() == Some(1) && said.contains("not set") {
            continue;
        }
        return Err(unreadable(if output.status.success() {
            "it printed no name".into()
        } else {
            format!("{}: {}", output.status, said.trim())
        }));
    }
    Ok(names)
}

/// Where `name` (lower case) appears in `line` as a whole word, ignoring
/// case: `laptop-kestrel` and `Kestrel` match `kestrel`, `danae` does
/// not match `dana`.
fn word_at(line: &str, name: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    lower.match_indices(name).map(|(i, _)| i).find(|&i| {
        !word(lower[..i].chars().next_back()) && !word(lower[i + name.len()..].chars().next())
    })
}

/// The first text in `line` that looks personal, with a little context: home
/// and temporary paths (also JSON-escaped, and in the `-Users-` encoding),
/// `.local` hosts, addresses, and this user's own login and home and this
/// machine's names, each as a whole word in any case.
fn personal(line: &str, own: &[String]) -> Option<String> {
    const MARKERS: [&str; 8] = [
        "/Users/",
        "/home/",
        "/root/",
        "\\\\Users\\\\",
        "-Users-",
        "~/",
        "/var/folders/",
        "/private/tmp/",
    ];
    let at = MARKERS
        .iter()
        .filter_map(|marker| line.find(marker))
        .chain(local_host_at(line))
        .chain(email_at(line))
        .chain(own.iter().filter_map(|name| word_at(line, name)))
        .min()?;
    let start = line[..at]
        .char_indices()
        .rev()
        .nth(20)
        .map_or(0, |(i, _)| i);
    Some(line[start..].chars().take(60).collect())
}

/// Where a `<host>.local` ends a word (before a quote, space, colon, slash…).
fn local_host_at(line: &str) -> Option<usize> {
    line.match_indices(".local")
        .find(|(i, _)| {
            line[i + 6..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
        .map(|(i, _)| i)
}

/// Where an address-like `name@host.tld` starts: the top-level domain is
/// letters, so a package id such as `serde@1.0.200` is not an address.
fn email_at(line: &str) -> Option<usize> {
    line.match_indices('@').find_map(|(at, _)| {
        let before = line[..at].chars().last()?;
        let host: String = line[at + 1..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        let tld = host.rsplit('.').next().unwrap_or_default();
        let named =
            host.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic());
        (before.is_alphanumeric() && named).then(|| {
            line[..at]
                .rfind(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_' || c == '-'))
                .map_or(0, |i| i + 1)
        })
    })
}

/// The audit: one row per ref (`ref`: its notes, lines and bytes), per
/// identity of the notes commits (`identity`), and per finding (`violation`:
/// `detail` the problem; `personal`: `detail` the sample).
impl TidyRows for Audit {
    fn header(&self) -> Vec<String> {
        columns(&[
            "section", "name", "kind", "tree", "run", "seq", "notes", "lines", "bytes", "detail",
        ])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        let mut rows = Vec::new();
        for r in &self.refs {
            rows.push(vec![
                cell("ref"),
                cell(&r.name),
                None,
                None,
                None,
                None,
                cell(r.notes),
                cell(r.lines),
                cell(r.bytes),
                None,
            ]);
        }
        for identity in &self.identities {
            let mut row = vec![cell("identity"), cell(identity)];
            row.extend([None, None, None, None, None, None, None, None]);
            rows.push(row);
        }
        for finding in &self.findings {
            rows.push(match finding {
                Finding::Violation {
                    kind,
                    tree,
                    problem,
                } => vec![
                    cell("violation"),
                    None,
                    cell(kind),
                    cell(tree),
                    None,
                    None,
                    None,
                    None,
                    None,
                    cell(problem),
                ],
                Finding::Personal {
                    kind,
                    tree,
                    run,
                    seq,
                    sample,
                } => vec![
                    cell("personal"),
                    None,
                    cell(kind),
                    cell(tree),
                    run.as_ref().map(ToString::to_string),
                    seq.map(|s| s.to_string()),
                    None,
                    None,
                    None,
                    cell(sample),
                ],
            });
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_name_the_audit_cannot_read_refuses_it_and_an_unset_one_does_not() {
        let refused =
            |asked: &[Lookup<'_>]| matches!(host_names(asked), Err(RemoteError::HostName { .. }));
        let missing = host_names(&[Lookup::required(&["git-evidence-no-such-program"])])
            .expect_err("a missing program")
            .to_string();
        assert!(
            missing.starts_with(
                "cannot read this machine's name (git-evidence-no-such-program: No such file"
            ) && !missing.contains("git:"),
            "names the program that failed, never git: {missing}"
        );
        assert!(refused(&[Lookup::required(&["true"])]), "no name printed");
        let unset: &[&str] = &["sh", "-c", "echo 'LocalHostName: not set'; exit 1"];
        assert!(
            refused(&[Lookup::required(unset)]),
            "required, so unset refuses"
        );
        assert_eq!(
            host_names(&[Lookup::may_be_unset(unset)]).expect("unset is an answer"),
            Vec::<String>::new()
        );
        let broken: &[&str] = &["sh", "-c", "echo boom >&2; exit 2"];
        assert!(
            refused(&[Lookup::may_be_unset(broken)]),
            "only 'not set' is unset"
        );
        assert_eq!(
            host_names(&[Lookup::required(&["echo", "box.example.lan"])]).expect("a name"),
            vec!["box".to_owned(), "box.example.lan".to_owned()]
        );
    }

    #[test]
    fn a_host_or_login_name_is_found_as_a_word_in_any_case() {
        let own = ["kestrel".to_owned(), "dana".to_owned()];
        assert!(personal(r#"{"machine":"Kestrel"}"#, &own).is_some());
        assert!(personal(r#"{"machine":"laptop-kestrel"}"#, &own).is_some());
        assert!(
            personal(r#"{"label":"danae"}"#, &own).is_none(),
            "not inside a word"
        );
        assert!(personal(r#"{"machine":"laptop"}"#, &own).is_none());
    }

    #[test]
    fn publishable_text_is_text_the_audit_never_flags() {
        // As the system's lookups give them: the name and its first label.
        let names = OwnNames::new(
            vec!["Dana".into()],
            vec!["Kestrel.local".into(), "Kestrel".into()],
        );
        assert!(names.publishable("crates/core/src/lib.rs"));
        for text in [
            "crates/dana/lib.rs",
            "docs/kestrel-notes.md",
            "/Users/someone/x",
            "a\\Users\\b",
            "box.local",
            "me@example.org",
        ] {
            assert!(!names.publishable(text), "{text}");
        }
    }
}
