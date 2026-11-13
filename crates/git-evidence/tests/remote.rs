//! `fetch` and `push` between stores (README "Guarantees": grow-only, idempotent):
//! evidence written in one store is read in another; two stores that appended
//! concurrently converge on the union; a refused push merges and retries; fetched
//! copies are never mistaken for branches; nothing is forced.

mod common;

use common::*;
use git_evidence::CommitId;
use git_evidence::Evidence;
use git_evidence::testing::push_as;
use git_evidence::{End, EventLine};
use git_process::Git;
use git_process::testing::hermetic;

use git_evidence::{RemoteError, UpdateKind};
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let status = hermetic(&mut Command::new("git"))
        .arg("-C")
        .arg(dir)
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=0"])
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

/// A bare remote with one landing, and `n` clones of it.
fn network(n: usize) -> (tempfile::TempDir, Vec<Evidence>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let seed = dir.path().join("seed");
    git(
        dir.path(),
        &["init", "-q", "--initial-branch=master", "seed"],
    );
    git(&seed, &["commit", "-q", "--allow-empty", "-m", "landing 0"]);
    git(dir.path(), &["clone", "-q", "--bare", "seed", "remote.git"]);
    let stores = (0..n)
        .map(|i| {
            let name = format!("clone{i}");
            git(dir.path(), &["clone", "-q", "remote.git", &name]);
            git(
                &dir.path().join(&name),
                &["config", "evidence.push", "true"],
            );
            // The notes commits a store writes carry the fixture's identity.
            git_process::testing::fixture_config(&Git::at(dir.path().join(&name).join(".git")))
                .expect("fixture config");
            Evidence::open(dir.path().join(name).join(".git")).trunk("refs/heads/master")
        })
        .collect();
    (dir, stores)
}

/// One local landing for checks that never use remote transport.
fn local() -> git_evidence::testing::Scratch {
    git_evidence::testing::Scratch::new()
        .expect("scratch")
        .landings(1)
        .expect("a landing")
}

fn landing(store: &Evidence) -> CommitId {
    let id = git_process::Git::at(store.git_dir())
        .resolve("refs/heads/master")
        .expect("resolve")
        .expect("master");
    CommitId::parse(id.as_str()).expect("commit")
}

fn lines_for(store: &Evidence, machine: &'static str) -> Vec<EventLine> {
    let shape = Shape {
        machine,
        ..Shape::default()
    };
    run_lines(
        open(&landing(store), &shape, &["parse_large"]),
        &[("parse_large", p99(88.0))],
        Some(End::Complete),
    )
}

fn write(store: &Evidence, lines: &[EventLine]) {
    store.record(&kind("perf"), lines).expect("record");
}

fn runs_seen(store: &Evidence) -> i64 {
    let rows = store.sql("SELECT count(*) FROM runs").expect("sql");
    match rows.rows[0][0] {
        git_evidence::SqlValue::Integer(n) => n,
        ref other => panic!("{other:?}"),
    }
}

#[test]
fn evidence_written_in_one_store_is_read_in_another() {
    let (_dir, stores) = network(2);
    let (bench, laptop) = (&stores[0], &stores[1]);
    write(bench, &lines_for(bench, "bench"));
    assert_eq!(
        push_as(bench, "origin", NO_OWN_NAMES, NO_OWN_NAMES)
            .expect("push")
            .refs[0]
            .change,
        UpdateKind::Created
    );
    assert_eq!(runs_seen(laptop), 0, "nothing until the laptop fetches");
    let fetched = laptop.fetch("origin").expect("fetch");
    assert_eq!(fetched.merged, vec!["perf".to_owned()]);
    assert_eq!(runs_seen(laptop), 1);
}

#[test]
fn two_stores_that_appended_concurrently_converge_on_the_union() {
    let (dir, stores) = network(2);
    let (a, b) = (&stores[0], &stores[1]);
    write(a, &lines_for(a, "bench"));
    write(b, &lines_for(b, "laptop"));
    push_as(a, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("first push");
    let second = push_as(b, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("second push");
    assert!(
        second
            .refs
            .iter()
            .all(|r| !matches!(r.change, UpdateKind::Refused(_))),
        "the refused push fetched, merged and retried: {second:?}"
    );
    assert_eq!(second.merged, vec!["perf".to_owned()]);
    a.fetch("origin").expect("fetch");
    assert_eq!(
        (runs_seen(a), runs_seen(b)),
        (2, 2),
        "grow-only: both hold the union"
    );
    let remote = Git::at(dir.path().join("remote.git"));
    let notes = "refs/notes/evidence/perf";
    assert_eq!(
        remote.resolve(notes).expect("remote"),
        git_process::Git::at(b.git_dir()).resolve(notes).expect("b"),
        "the remote moved by fast-forward to the merge: nothing forced"
    );
}

#[test]
fn the_same_run_through_two_stores_is_one_run() {
    // Idempotent: an import (or any batch) delivered in two stores is one run after merging.
    let (_dir, stores) = network(2);
    let (a, b) = (&stores[0], &stores[1]);
    let lines = lines_for(a, "bench");
    write(a, &lines);
    write(b, &lines);
    push_as(a, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("push");
    push_as(b, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("push merges");
    assert_eq!(runs_seen(b), 1);
}

#[test]
fn fetched_copies_are_never_mistaken_for_branches() {
    let (dir, stores) = network(2);
    write(&stores[0], &lines_for(&stores[0], "bench"));
    push_as(&stores[0], "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("push");
    stores[1].fetch("origin").expect("fetch");
    let branches = hermetic(&mut Command::new("git"))
        .arg("-C")
        .arg(dir.path().join("clone1"))
        .args(["branch", "-r"])
        .output()
        .expect("git branch");
    let listed = String::from_utf8_lossy(&branches.stdout);
    assert!(!listed.contains("evidence"), "{listed}");
    let notes = hermetic(&mut Command::new("git"))
        .arg("-C")
        .arg(dir.path().join("clone1"))
        .args(["notes", "--ref", "remotes/origin/evidence/perf", "list"])
        .output()
        .expect("git notes");
    assert!(
        notes.status.success() && !notes.stdout.is_empty(),
        "git notes reads the fetched copy"
    );
}

#[test]
fn a_path_or_url_is_refused_up_front_naming_the_fix() {
    let (dir, stores) = network(1);
    let error = stores[0]
        .push(dir.path().join("remote.git").to_str().expect("utf8"))
        .expect_err("refused");
    assert!(error.to_string().contains("git remote add"), "{error}");
}

#[test]
fn nothing_is_pushed_until_the_store_enables_it_and_a_dry_run_sends_nothing() {
    // Pushed evidence is permanent, so pushing is off by default,
    // and a dry run shows what would go without sending or merging anything.
    let (dir, stores) = network(1);
    let store = &stores[0];
    write(store, &lines_for(store, "bench"));
    git(
        &dir.path().join("clone0"),
        &["config", "--unset", "evidence.push"],
    );
    assert!(matches!(
        push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES),
        Err(git_evidence::Error::Remote(RemoteError::PushNotEnabled))
    ));
    let planned = store.push_dry_run("origin").expect("dry run");
    assert!(!planned.refs.is_empty(), "{planned:?}");
    let remote_refs = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(dir.path().join("remote.git"))
        .args(["for-each-ref", "refs/notes/"])
        .output()
        .expect("git");
    assert!(remote_refs.stdout.is_empty(), "a dry run sent nothing");
    git(
        &dir.path().join("clone0"),
        &["config", "evidence.push", "true"],
    );
    push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("enabled");
}

#[test]
fn a_push_whose_audit_finds_anything_sends_nothing() {
    // Every push is an audited push: pushed evidence is permanent in every
    // clone, so a leak stops it before anything leaves.
    let (dir, stores) = network(1);
    let store = &stores[0];
    let mut leaky = lines_for(store, "bench");
    if let git_evidence::Event::Open(open) = &mut leaky[0].event {
        open.environment
            .descriptor
            .insert("checkout".into(), serde_json::json!("/Users/someone/code"));
    }
    write(store, &leaky);
    assert!(matches!(
        push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES),
        Err(git_evidence::Error::Remote(RemoteError::AuditFindings(1)))
    ));
    let remote_refs = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(dir.path().join("remote.git"))
        .args(["for-each-ref", "refs/notes/"])
        .output()
        .expect("git");
    assert!(remote_refs.stdout.is_empty(), "nothing was sent");
    // Removing the note does not remove it from the history a push sends, and
    // a fetched copy of another remote holding it does not mean origin has it.
    let tree = git_process::Git::at(store.git_dir())
        .tree_of(landing(store).object())
        .expect("tree");
    let copied = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(store.git_dir())
        .args([
            "update-ref",
            "refs/notes/remotes/other/evidence/perf",
            "refs/notes/evidence/perf",
        ])
        .status()
        .expect("git");
    assert!(copied.success());
    let removed = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(store.git_dir())
        .args(["notes", "--ref", "refs/notes/evidence/perf", "remove"])
        .arg(tree.as_str())
        .status()
        .expect("git");
    assert!(removed.success());
    assert!(matches!(
        push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES),
        Err(git_evidence::Error::Remote(RemoteError::AuditFindings(1)))
    ));
}

#[test]
fn distinct_unreadable_lines_are_each_a_finding() {
    let scratch = local();
    let store = &scratch.evidence();
    let tree = git_process::Git::at(store.git_dir())
        .tree_of(landing(store).object())
        .expect("tree");
    let status = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(store.git_dir())
        .args([
            "notes",
            "--ref",
            "refs/notes/evidence/perf",
            "add",
            "-m",
            "{\"a\":1}\n{\"b\":2}",
        ])
        .arg(tree.as_str())
        .status()
        .expect("git");
    assert!(status.success());
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    let violations = audit
        .findings
        .iter()
        .filter(|f| matches!(f, git_evidence::Finding::Violation { .. }))
        .count();
    assert_eq!(violations, 2, "{audit:?}");
}

/// No own names: each leak must be caught by its own detector (a path marker,
/// an address, a `.local` host), not by a name that happens to occur in the
/// fixture, and nothing depends on the machine running the tests (the role
/// `bench` is a real host name on bench).
const NO_OWN_NAMES: &[&str] = &[];

#[test]
fn a_declared_role_that_is_also_this_hosts_name_is_public() {
    let scratch = local();
    let store = &scratch.evidence();
    write(store, &lines_for(store, "bench"));
    assert!(
        git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES)
            .expect("audit")
            .findings
            .is_empty()
    );
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, &["Bench"]).expect("audit");
    assert!(audit.findings.is_empty(), "{audit:?}");
}

#[test]
fn declared_names_do_not_exempt_other_personal_text() {
    for (name, machine, label) in [
        ("gates", "dana", "dana"),
        ("dana", "laptop", "dana"),
        ("gates", "laptop", "/home/dana"),
    ] {
        let scratch = local();
        let store = &scratch.evidence();
        let mut lines = lines_for(store, machine);
        let git_evidence::Event::Open(open) = &mut lines[0].event else {
            panic!("fixture open");
        };
        open.suite.params.insert("label".into(), label.into());
        store.record(&kind(name), &lines).expect("record");
        let audit = git_evidence::testing::audit_as(store, &["dana"], &["gates"]).expect("audit");
        assert_eq!(audit.findings.len(), 1, "{label}: {audit:?}");
    }
}

#[test]
fn a_host_name_is_private_until_the_store_declares_it() {
    let scratch = local();
    let store = &scratch.evidence();
    let mut lines = lines_for(store, "laptop");
    let git_evidence::Event::Open(open) = &mut lines[0].event else {
        panic!("fixture open");
    };
    open.suite.params.insert("label".into(), "gates".into());
    write(store, &lines);
    let private = git_evidence::testing::audit_as(store, NO_OWN_NAMES, &["gates"]).expect("audit");
    assert_eq!(private.findings.len(), 1, "{private:?}");
    git(
        store.git_dir(),
        &["config", "--local", "evidence.machine", "gates"],
    );
    let public = git_evidence::testing::audit_as(store, NO_OWN_NAMES, &["gates"]).expect("audit");
    assert!(public.findings.is_empty(), "{public:?}");
}

#[test]
fn public_roles_are_derived_across_all_audited_refs() {
    let scratch = local();
    let store = &scratch.evidence();
    let mut lines = lines_for(store, "laptop");
    let git_evidence::Event::Open(open) = &mut lines[0].event else {
        panic!("fixture open");
    };
    open.suite.params.insert("label".into(), "gates".into());
    store.record(&kind("aaa"), &lines).expect("first kind");
    store
        .record(&kind("zzz"), &lines_for(store, "gates"))
        .expect("later kind");
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, &["gates"]).expect("audit");
    assert!(audit.findings.is_empty(), "{audit:?}");
}

/// Appends `text` to the newest landing's perf note with Git itself, past
/// `record`'s checks: what another tool, or an older version, could have
/// written.
fn append_unchecked(store: &Evidence, text: &str) {
    let tree = git_process::Git::at(store.git_dir())
        .tree_of(landing(store).object())
        .expect("tree");
    let status = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(store.git_dir())
        .args([
            "notes",
            "--ref",
            "refs/notes/evidence/perf",
            "append",
            "--no-separator",
            "-m",
        ])
        .arg(text)
        .arg(tree.as_str())
        .status()
        .expect("git");
    assert!(status.success());
}

#[test]
fn an_invalid_recorded_machine_does_not_declare_a_public_name() {
    let scratch = local();
    let store = &scratch.evidence();
    let lines = lines_for(store, "alpha/beta");
    assert!(
        store.record(&kind("perf"), &lines).is_err(),
        "record refuses a machine that is not a machine name"
    );
    let text = lines
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect::<Vec<_>>()
        .join("\n");
    append_unchecked(store, &text);
    let audit = git_evidence::testing::audit_as(store, &[], &["alpha/beta"]).expect("audit");
    assert!(
        audit
            .findings
            .iter()
            .any(|f| matches!(f, git_evidence::Finding::Personal { .. })),
        "the machine is not declared public: {audit:?}"
    );
}

#[test]
fn the_audit_names_what_must_not_be_pushed() {
    // Before the first push, see sizes, unreadable runs and anything
    // personal (a home path, a .local host, an address).
    let scratch = local();
    let store = &scratch.evidence();
    write(store, &lines_for(store, "bench"));
    let clean = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    assert!(clean.findings.is_empty(), "{clean:?}");
    assert_eq!(clean.refs.len(), 1);
    assert!(clean.refs[0].bytes > 0);
    let mut leaky = lines_for(store, "bench");
    if let git_evidence::Event::Open(open) = &mut leaky[0].event {
        open.environment
            .descriptor
            .insert("host".into(), serde_json::json!("Kestrel.local"));
        open.environment
            .descriptor
            .insert("checkout".into(), serde_json::json!("/Users/someone/code"));
        open.environment
            .descriptor
            .insert("owner".into(), serde_json::json!("someone@example.com"));
    }
    write(store, &leaky);
    let mut old = lines_for(store, "laptop");
    for line in &mut old {
        line.v = 1;
    }
    let text = old
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect::<Vec<_>>()
        .join("\n");
    append_unchecked(store, &text);
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    let personal = audit
        .findings
        .iter()
        .filter(|f| matches!(f, git_evidence::Finding::Personal { .. }))
        .count();
    assert_eq!(personal, 1, "one line, flagged once: {audit:?}");
    assert!(
        audit
            .findings
            .iter()
            .any(|f| matches!(f, git_evidence::Finding::Violation { .. })),
        "the v1 run: {audit:?}"
    );
}

#[test]
fn a_dry_run_and_a_push_run_no_hooks() {
    // Evidence refs are not code: the store's pre-push hook (bd, LFS) must not
    // run on them, and cannot refuse them.
    let (dir, stores) = network(1);
    let store = &stores[0];
    write(store, &lines_for(store, "bench"));
    let hooks = dir.path().join("clone0/.git/hooks");
    std::fs::create_dir_all(&hooks).expect("hooks");
    let marker = dir.path().join("hook-ran");
    let hook = hooks.join("pre-push");
    std::fs::write(
        &hook,
        format!("#!/bin/sh\ntouch {}\nexit 1\n", marker.display()),
    )
    .expect("hook");
    let mut permissions = std::fs::metadata(&hook).expect("meta").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&hook, permissions).expect("chmod");
    store.push_dry_run("origin").expect("dry run");
    push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("push");
    assert!(!marker.exists(), "no hook ran");
}

#[test]
fn the_audit_sees_home_paths_hosts_and_addresses_but_not_package_ids() {
    let scratch = local();
    let store = &scratch.evidence();
    let mut lines = lines_for(store, "bench");
    if let git_evidence::Event::Open(open) = &mut lines[0].event {
        open.environment.descriptor.insert(
            "pkg".into(),
            serde_json::json!("react@18.3.1 serde@1.0.200"),
        );
    }
    write(store, &lines);
    let clean = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    assert!(
        clean.findings.is_empty(),
        "package ids are not addresses: {clean:?}"
    );
    assert!(
        !clean.identities.is_empty(),
        "the notes commits' identities are listed"
    );
    for leak in [
        "/root/build",
        "C:\\Users\\someone",
        "/private/tmp/x",
        "laptop.local:22",
    ] {
        let scratch = local();
        let store = &scratch.evidence();
        let mut lines = lines_for(store, "bench");
        if let git_evidence::Event::Open(open) = &mut lines[0].event {
            open.environment
                .descriptor
                .insert("leak".into(), serde_json::json!(leak));
        }
        write(store, &lines);
        let audit =
            git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
        assert!(
            audit
                .findings
                .iter()
                .any(|f| matches!(f, git_evidence::Finding::Personal { run: Some(_), .. })),
            "{leak}: {audit:?}"
        );
    }
}

#[test]
fn an_incomplete_clone_cannot_be_audited() {
    // A shallow clone hides history a push would send: a clean audit of it
    // would be a partial answer passed off as complete.
    let dir = tempfile::tempdir().expect("tempdir");
    let seed = dir.path().join("seed");
    git(
        dir.path(),
        &["init", "-q", "--initial-branch=master", "seed"],
    );
    git(&seed, &["commit", "-q", "--allow-empty", "-m", "landing 0"]);
    git(&seed, &["commit", "-q", "--allow-empty", "-m", "landing 1"]);
    let url = format!("file://{}", seed.display());
    git(
        dir.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );
    let store = Evidence::open(dir.path().join("shallow").join(".git"));
    let refused = store.audit().expect_err("a shallow clone is refused");
    assert!(
        refused.to_string().contains("--unshallow"),
        "the refusal names the fix: {refused}"
    );
}

#[test]
fn the_audit_reads_this_machines_names_under_a_launchd_path() {
    // A scheduled push from launchd runs with PATH /usr/bin:/bin: every name
    // lookup the audit makes must run there, or every scheduled push refuses
    // (a regression found the night it landed).
    let scratch = local();
    // git's own directory, then launchd's PATH: nothing else
    // the audit needs may come from the caller's shell.
    let git = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .find(|dir| dir.join("git").is_file())
        .expect("git on PATH");
    let path = std::env::join_paths([git, "/usr/bin".into(), "/bin".into()]).expect("PATH");
    let output = scratch
        .command(env!("CARGO_BIN_EXE_git-evidence"))
        .arg("audit")
        .env("PATH", path)
        .output()
        .expect("audit");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_notes_commits_carry_the_fixtures_identity_under_a_runners_own() {
    // A build machine's own identity (`bench <bench@localhost>`, from its global
    // configuration) and home once reached these tests' audits; a fixture
    // store commits as the fixture whatever the runner is.
    let (dir, stores) = network(1);
    let store = &stores[0];
    let home = tempfile::tempdir().expect("runner's home");
    let config = home.path().join(".gitconfig");
    std::fs::write(
        &config,
        "[user]\n\tname = bench\n\temail = bench@localhost\n[commit]\n\tgpgSign = true\n[gpg]\n\tprogram = false\n",
    )
    .expect("runner's config");
    let input: String = lines_for(store, "bench")
        .iter()
        .map(|l| serde_json::to_string(l).expect("JSON") + "\n")
        .collect();
    // The command line as the runner starts it: its home, its configuration,
    // its login; identity from configuration, not variables.
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-evidence"));
    git_process::isolated(&mut command)
        .current_dir(dir.path().join("clone0"))
        .env("HOME", home.path())
        .env("USER", "bench")
        .env("LOGNAME", "bench")
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    for name in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_EVIDENCE_GIT_DIR",
    ] {
        command.env_remove(name);
    }
    let mut child = command
        .args(["record", "perf"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("CLI");
    std::io::Write::write_all(&mut child.stdin.take().expect("stdin"), input.as_bytes())
        .expect("write");
    let recorded = child.wait_with_output().expect("CLI");
    assert!(recorded.status.success(), "{recorded:?}");
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    assert!(audit.findings.is_empty(), "{audit:?}");
    let fixture = format!(
        "{} <{}>",
        git_process::testing::NAME,
        git_process::testing::EMAIL
    );
    assert_eq!(audit.identities, vec![fixture], "{audit:?}");
}

#[test]
fn a_ref_the_remote_holds_already_is_neither_judged_nor_sent() {
    // A push costs what changed since the remote took this store's evidence:
    // a ref the remote holds at this store's commit is not read again.
    let (dir, stores) = network(1);
    let store = &stores[0];
    let mut leaky = lines_for(store, "bench");
    if let git_evidence::Event::Open(open) = &mut leaky[0].event {
        open.environment
            .descriptor
            .insert("checkout".into(), serde_json::json!("/Users/someone/code"));
    }
    write(store, &leaky);
    // Published past the audit, as an older version could have.
    git(
        &dir.path().join("clone0"),
        &[
            "push",
            "-q",
            "--no-verify",
            "origin",
            "refs/notes/evidence/perf:refs/notes/evidence/perf",
        ],
    );
    store
        .record(&kind("checks"), &lines_for(store, "bench"))
        .expect("record");
    let pushed = push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES)
        .expect("only what the remote lacks is judged");
    let change = |reference: &str| {
        pushed
            .refs
            .iter()
            .find(|r| r.reference == reference)
            .map(|r| r.change.clone())
    };
    assert_eq!(
        change("refs/notes/evidence/perf"),
        Some(UpdateKind::UpToDate)
    );
    assert_eq!(
        change("refs/notes/evidence/checks"),
        Some(UpdateKind::Created)
    );
    // The audit of the whole store still names what was published.
    let audit = git_evidence::testing::audit_as(store, NO_OWN_NAMES, NO_OWN_NAMES).expect("audit");
    assert_eq!(audit.findings.len(), 1, "{audit:?}");
}

#[test]
fn a_note_sent_is_judged_with_the_version_the_remote_holds() {
    // A push sends only what the remote lacks, but a line sent is judged
    // beside the lines the remote already publishes for the same tree.
    let (_dir, stores) = network(1);
    let store = &stores[0];
    let lines = lines_for(store, "bench");
    write(store, &lines);
    push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES).expect("a clean push");
    // The note rewritten so that its seq 1 differs from the one published:
    // alone it folds; beside the published version it does not.
    let mut rewritten = lines.clone();
    rewritten[1].time = "2000-01-01T00:00:00.000Z".into();
    let text = rewritten
        .iter()
        .map(|l| serde_json::to_string(l).expect("json"))
        .collect::<Vec<_>>()
        .join("\n");
    let tree = git_process::Git::at(store.git_dir())
        .tree_of(landing(store).object())
        .expect("tree");
    let status = hermetic(&mut Command::new("git"))
        .arg("--git-dir")
        .arg(store.git_dir())
        .args([
            "notes",
            "--ref",
            "refs/notes/evidence/perf",
            "add",
            "-f",
            "-m",
        ])
        .arg(&text)
        .arg(tree.as_str())
        .status()
        .expect("git");
    assert!(status.success());
    assert!(
        matches!(
            push_as(store, "origin", NO_OWN_NAMES, NO_OWN_NAMES),
            Err(git_evidence::Error::Remote(RemoteError::AuditFindings(n))) if n > 0
        ),
        "the rewritten run disagrees with the published one"
    );
}
