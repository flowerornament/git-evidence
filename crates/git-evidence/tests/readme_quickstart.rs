//! The README's quick start runs verbatim: each `$` command of its console
//! block runs in a fresh repository, with `git-evidence` on `PATH` and fixed
//! commit dates, and prints exactly the output the README shows under it.

use std::path::Path;
use std::process::Command;

/// One command of the block and the output the README shows for it.
struct Step {
    command: String,
    output: String,
}

/// The console block under "## Quick start": a line starting `$ ` begins a
/// command, a here-document's lines through its `EOF` belong to it, and the
/// lines up to the next command are its output.
fn steps(readme: &str) -> Vec<Step> {
    let section = &readme[readme.find("## Quick start").expect("a quick start")..];
    let block = &section[section.find("```console\n").expect("a console block") + 11..];
    let block = &block[..block.find("```").expect("the block's end")];
    let mut steps: Vec<Step> = Vec::new();
    let mut heredoc = false;
    for line in block.lines() {
        if heredoc {
            let step = steps.last_mut().expect("a command");
            step.command.push('\n');
            step.command.push_str(line);
            heredoc = line != "EOF";
        } else if let Some(command) = line.strip_prefix("$ ") {
            heredoc = command.ends_with("<<EOF");
            steps.push(Step {
                command: command.to_owned(),
                output: String::new(),
            });
        } else {
            let step = steps.last_mut().expect("output follows a command");
            step.output.push_str(line);
            step.output.push('\n');
        }
    }
    steps
}

fn run(step: &Step, repo: &Path, home: &Path) -> std::process::Output {
    let bin = Path::new(env!("CARGO_BIN_EXE_git-evidence"))
        .parent()
        .expect("the binary's directory");
    let path = std::env::join_paths(std::iter::once(bin.to_owned()).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .expect("PATH");
    let date = "2026-10-08T03:00:00Z";
    Command::new("bash")
        .args(["-c", &step.command])
        .current_dir(repo)
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("bash")
}

fn check(readme: &str) -> Result<usize, String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let (repo, home) = (dir.path().join("demo"), dir.path().join("home"));
    for path in [&repo, &home] {
        std::fs::create_dir(path).expect("a directory");
    }
    let steps = steps(readme);
    for step in &steps {
        let output = run(step, &repo, &home);
        let printed = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || printed != step.output {
            return Err(format!(
                "$ {}\nexpected:\n{}printed ({}):\n{printed}{}",
                step.command,
                step.output,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    Ok(steps.len())
}

#[test]
fn the_quick_start_runs_verbatim() {
    let readme = include_str!("../README.md");
    let ran = check(readme).unwrap_or_else(|why| panic!("{why}"));
    assert!(ran >= 8, "the quick start runs {ran} commands");
}

/// The test reads the README's output, so a changed output fails it.
#[test]
fn a_changed_output_fails_the_quick_start() {
    let readme = include_str!("../README.md");
    let changed = readme.replacen("written  already_present", "written  present_now", 1);
    assert_ne!(changed, readme);
    assert!(check(&changed).is_err());
}
