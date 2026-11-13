//! Revisions on the command line: a malformed command exits 1, and inside a
//! jj workspace `HEAD` is the workspace's `@-`, never another workspace's
//! checkout or an outer repository's.

#[test]
fn a_malformed_command_could_not_be_asked_and_exits_1_with_nothing_on_stdout() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let cli = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_git-evidence"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .expect("run the CLI")
    };
    for args in [&["--csv", "no-such-question"][..], &["trend"][..]] {
        let output = cli(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(!output.stderr.is_empty(), "{args:?}: the fix is named");
    }
    let help = cli(&["--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    assert!(!help.stdout.is_empty(), "{help:?}");
}

#[test]
fn a_jj_workspace_inside_another_repository_never_answers_with_that_repositorys_revisions() {
    // A jj workspace sits inside an outer Git repository with a commit. HEAD
    // is the workspace's @- (here the root, no commit), never the outer HEAD.
    let root = tempfile::tempdir().expect("tempdir");
    let run = |program: &str, dir: &std::path::Path, args: &[&str]| {
        git_process::testing::hermetic(&mut std::process::Command::new(program))
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_EVIDENCE_GIT_DIR")
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@t")
            .output()
            .expect("run")
    };
    let outer = root.path();
    assert!(run("git", outer, &["init", "--quiet"]).status.success());
    let committed = run(
        "git",
        outer,
        &["commit", "--quiet", "--allow-empty", "-m", "outer"],
    );
    assert!(committed.status.success(), "{committed:?}");
    let outer_head =
        String::from_utf8(run("git", outer, &["rev-parse", "HEAD"]).stdout).expect("utf-8");
    let initialized = run("jj", outer, &["git", "init", "workspace"]);
    assert!(initialized.status.success(), "{initialized:?}");
    let workspace = outer.join("workspace");
    let answer = run(
        env!("CARGO_BIN_EXE_git-evidence"),
        &workspace,
        &["log", "HEAD"],
    );
    let said = String::from_utf8_lossy(&answer.stderr).into_owned()
        + &String::from_utf8_lossy(&answer.stdout);
    assert!(!answer.status.success(), "{said}");
    assert!(said.contains("HEAD names no commit here"), "{said}");
    assert!(
        !said.contains(outer_head.trim()),
        "never the outer repository: {said}"
    );
}

#[test]
fn a_second_jj_workspace_reads_head_as_its_own_parent_never_the_shared_head() {
    // A colocated repository whose shared Git store has a HEAD (the first
    // workspace's checkout), and a second jj workspace. In the second, HEAD
    // is its own @-, never the shared HEAD; any other Git revision jj cannot
    // resolve (`origin/master`, `HEAD~0`) is read from the store.
    let root = tempfile::tempdir().expect("tempdir");
    let run = |program: &str, dir: &std::path::Path, args: &[&str]| {
        git_process::testing::hermetic(&mut std::process::Command::new(program))
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_EVIDENCE_GIT_DIR")
            .env("JJ_CONFIG", "/dev/null")
            .env("JJ_USER", "t")
            .env("JJ_EMAIL", "t@t")
            .output()
            .expect("run")
    };
    let main = root.path().join("main");
    std::fs::create_dir(&main).expect("main");
    assert!(run("git", &main, &["init", "--quiet"]).status.success());
    let committed = run(
        "git",
        &main,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    assert!(committed.status.success(), "{committed:?}");
    let head = String::from_utf8(run("git", &main, &["rev-parse", "HEAD"]).stdout).expect("utf-8");
    let head = head.trim();
    assert!(
        run(
            "git",
            &main,
            &["update-ref", "refs/remotes/origin/master", head]
        )
        .status
        .success()
    );
    let colocated = run("jj", &main, &["git", "init", "--colocate"]);
    assert!(colocated.status.success(), "{colocated:?}");
    let second = root.path().join("second");
    let added = run(
        "jj",
        &main,
        &["workspace", "add", second.to_str().expect("utf-8 path")],
    );
    assert!(added.status.success(), "{added:?}");
    let ask = |args: &[&str]| {
        let answer = run(env!("CARGO_BIN_EXE_git-evidence"), &second, args);
        let said = String::from_utf8_lossy(&answer.stderr).into_owned()
            + &String::from_utf8_lossy(&answer.stdout);
        (answer.status.success(), said)
    };
    // The first workspace moves on to a commit with another tree; the shared
    // HEAD follows it, the second workspace's @- stays at `first`.
    std::fs::write(main.join("file"), "x").expect("a file");
    let moved = run("jj", &main, &["commit", "-m", "moved"]);
    assert!(moved.status.success(), "{moved:?}");
    let first_tree =
        String::from_utf8(run("git", &main, &["rev-parse", &format!("{head}^{{tree}}")]).stdout)
            .expect("utf-8");
    let shared_head =
        String::from_utf8(run("git", &main, &["rev-parse", "HEAD^{tree}"]).stdout).expect("utf-8");
    assert_ne!(first_tree, shared_head, "the shared HEAD moved on");
    for rev in ["HEAD", "HEAD~0", "HEAD^0"] {
        let (ok, said) = ask(&["log", rev]);
        assert!(!ok, "{said}");
        assert!(
            said.contains(&format!(
                "nothing is recorded on tree {}",
                first_tree.trim()
            )),
            "{rev} is this workspace's @-: {said}"
        );
    }
    // Every other spelling of Git's HEAD or its reflog would read the shared
    // store's HEAD: refused, exit 1, naming what to write instead.
    for rev in ["HEAD@{0}", "@{0}", "@^0", "head", "Head", "HEAD~1@{0}"] {
        let answer = run(env!("CARGO_BIN_EXE_git-evidence"), &second, &["log", rev]);
        let said = String::from_utf8_lossy(&answer.stderr);
        assert_eq!(answer.status.code(), Some(1), "{rev}: {said}");
        assert!(answer.stdout.is_empty(), "{rev}");
        assert!(
            said.contains("another workspace's checkout") && said.contains("@-"),
            "{rev}: {said}"
        );
        assert!(!said.contains(shared_head.trim()), "{rev}: {said}");
    }
    let git_spelling = ask(&["--csv", "log", "origin/master"]);
    let jj_spelling = ask(&["--csv", "log", "master@origin"]);
    // Nothing is recorded on it: both name the same commit's tree.
    assert!(
        git_spelling.1.contains("nothing is recorded on tree"),
        "{}",
        git_spelling.1
    );
    assert_eq!(git_spelling, jj_spelling, "one commit, two spellings");
    // A full ref name that only contains the letters `head` is read, never
    // refused as the shared HEAD.
    assert!(
        run("git", &main, &["update-ref", "refs/heads/released", head])
            .status
            .success()
    );
    let (ok, said) = ask(&["log", "refs/heads/released"]);
    assert!(!ok, "{said}");
    assert!(
        said.contains(&format!(
            "nothing is recorded on tree {}",
            first_tree.trim()
        )),
        "refs/heads/released is read: {said}"
    );
    // Only a branch name is respelled, never an expression: a range stays as
    // asked, and jj refuses it (respelled, `..master@origin` would name the one
    // commit here).
    let (ok, said) = ask(&["log", "origin/..master"]);
    assert!(!ok, "{said}");
    assert!(!said.contains("@origin"), "never respelled: {said}");
}
