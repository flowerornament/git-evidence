//! The manual is rendered from the command line itself: every verb `--help`
//! lists has its own page and an entry on the root page, the hidden `manual`
//! verb has neither, and the root page carries the exit status `--help`
//! prints (`src/cli/manual.rs`).

use std::process::Command;

fn git_evidence(args: &[&std::ffi::OsStr]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-evidence"))
        .args(args)
        .output()
        .expect("run git-evidence")
}

/// The verbs `--help` lists, in its order (clap's own `help` aside).
fn verbs() -> Vec<String> {
    let help = git_evidence(&["--help".as_ref()]);
    assert!(help.status.success(), "{help:?}");
    let help = String::from_utf8(help.stdout).expect("help is UTF-8");
    let commands = help
        .split_once("Commands:\n")
        .expect("--help lists the verbs")
        .1;
    // A verb line is indented two spaces. A wrapped description continues on
    // deeper-indented lines: clap wraps help when a dependency in the same
    // build enables its `wrap_help` feature.
    commands
        .lines()
        .take_while(|line| line.starts_with("  "))
        .filter(|line| !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|verb| *verb != "help")
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_verb_the_command_line_lists_has_a_page_and_the_root_page_names_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let written = git_evidence(&["manual".as_ref(), dir.path().as_os_str()]);
    assert!(written.status.success(), "{written:?}");
    let verbs = verbs();
    assert!(
        verbs.len() >= 10,
        "the verbs were read from --help: {verbs:?}"
    );
    let root = std::fs::read_to_string(dir.path().join("git-evidence.1")).expect("the root page");
    for verb in &verbs {
        let page = dir.path().join(format!("git-evidence-{verb}.1"));
        let page = std::fs::read_to_string(&page).unwrap_or_else(|_| panic!("no page for {verb}"));
        assert!(
            page.starts_with(".ie") && page.contains(&format!(".TH git-evidence-{verb} 1")),
            "{verb}'s page is a man page titled for it"
        );
        assert!(
            root.contains(&format!("git\\-evidence\\-{verb}(1)")),
            "the root page names {verb}"
        );
    }
    let mut pages: Vec<String> = std::fs::read_dir(dir.path())
        .expect("the manual's directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    pages.sort();
    let mut expected: Vec<String> = verbs
        .iter()
        .map(|verb| format!("git-evidence-{verb}.1"))
        .collect();
    expected.push("git-evidence.1".to_owned());
    expected.sort();
    assert_eq!(
        pages, expected,
        "one page per listed verb and the root page, nothing else"
    );
}

#[test]
fn the_root_page_carries_the_exit_status_help_prints() {
    let dir = tempfile::tempdir().expect("tempdir");
    let written = git_evidence(&["manual".as_ref(), dir.path().as_os_str()]);
    assert!(written.status.success(), "{written:?}");
    let root = std::fs::read_to_string(dir.path().join("git-evidence.1")).expect("the root page");
    let help = String::from_utf8(git_evidence(&["--help".as_ref()]).stdout).expect("UTF-8");
    assert!(root.contains(".SH \"EXIT STATUS\""), "{root}");
    for code in ["0", "1", "2"] {
        assert!(
            help.contains(&format!("\n  {code}  ")),
            "--help explains exit {code}"
        );
        assert!(
            root.contains(&format!(".TP\n\\fB{code}\\fR")),
            "the page explains exit {code}"
        );
    }
}

#[test]
fn the_manual_needs_no_repository_and_refuses_a_missing_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manual = |out: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_git-evidence"))
            .arg("manual")
            .arg(out)
            .current_dir(dir.path())
            .env("GIT_CEILING_DIRECTORIES", dir.path())
            .output()
            .expect("run git-evidence")
    };
    let written = manual(dir.path());
    assert!(
        written.status.success(),
        "outside any repository: {written:?}"
    );
    assert!(dir.path().join("git-evidence.1").exists());
    let missing = dir.path().join("absent");
    let refused = manual(&missing);
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert!(!missing.exists());
}

/// `sql --help` names every public view, from the library's own list, so a
/// reader learns the schema from the tool rather than the README alone.
#[test]
fn sql_help_names_every_public_view() {
    let help = git_evidence(&["sql".as_ref(), "--help".as_ref()]);
    assert!(help.status.success(), "{help:?}");
    let help = String::from_utf8(help.stdout).expect("help is UTF-8");
    let listed = help
        .split_once("Public views:\n")
        .expect("sql --help lists the public views")
        .1;
    for view in git_evidence::PUBLIC_VIEWS {
        assert!(
            listed.lines().any(|line| line.trim() == view),
            "sql --help does not list {view}:\n{help}"
        );
    }
}
