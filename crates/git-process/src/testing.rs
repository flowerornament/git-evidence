//! A throwaway repository for tests of code that reads Git (feature `testing`),
//! and the Git a fixture runs: the same on every machine, whatever identity,
//! home and configuration the runner has. [`crate::START_FAILURE_DIR`] names the
//! directory where [`StartFailureLog`] writes a test
//! process's retried starts.
//!
//! ```text
//!   runner's environment ──► hermetic(command) ──► git with no global or system
//!                                                  config, the fixture's identity
//!   code under test's git ──► fixture_config(repo) ──► the repository's own config
//!                                                      names the fixture's identity
//! ```

use crate::{Git, GitError, StartFailureLog, isolated, observe_start_failures, start};
use std::process::Command;

/// The name every fixture commits as: no runner's, so a test may name it.
pub const NAME: &str = "Fixture";

/// The address every fixture commits as.
pub const EMAIL: &str = "fixture@example.invalid";

/// `command` as a fixture runs Git: [`isolated`], reading no global or system
/// configuration (a runner's identity, signing, hooks or templates), and
/// committing as [`NAME`] `<`[`EMAIL`]`>`.
pub fn hermetic(command: &mut Command) -> &mut Command {
    isolated(command)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_AUTHOR_NAME", NAME)
        .env("GIT_AUTHOR_EMAIL", EMAIL)
        .env("GIT_COMMITTER_NAME", NAME)
        .env("GIT_COMMITTER_EMAIL", EMAIL)
}

/// Writes the fixture's identity, and no signing, into the repository's own
/// configuration, which outranks the runner's global and system files: Git
/// that the code under test starts with the runner's environment then commits
/// as the fixture and never asks for a key. A runner's `GIT_AUTHOR_*` or
/// `GIT_COMMITTER_*` variables still win over it, as they win over any file.
///
/// # Errors
/// Git refuses to write the configuration.
pub fn fixture_config(git: &Git) -> Result<(), GitError> {
    for (key, value) in [
        ("user.name", NAME),
        ("user.email", EMAIL),
        ("commit.gpgSign", "false"),
        ("tag.gpgSign", "false"),
    ] {
        git.run(&["config", "--local", key, value], None)?;
    }
    Ok(())
}

/// A repository in a temporary directory with one empty commit, `base`, on
/// `master`, made by [`hermetic`] Git and given [`fixture_config`]. The
/// directory is removed when the returned guard drops.
///
/// # Panics
/// When the directory cannot be made or Git refuses to initialise it: a test
/// fixture has no caller to refuse to.
#[must_use]
pub fn repo() -> (tempfile::TempDir, Git) {
    // A caller-installed observer takes precedence over fixture diagnostics.
    let _ = observe_start_failures::<StartFailureLog>();
    let dir = tempfile::tempdir().expect("tempdir");
    let status = start(
        hermetic(&mut Command::new("git"))
            .args(["init", "-q", "--initial-branch=master"])
            .arg(dir.path()),
    )
    .expect("start git init")
    .wait()
    .expect("git init");
    assert!(status.success());
    let git = Git::at(dir.path().join(".git"));
    fixture_config(&git).expect("fixture config");
    let tree = git.write_tree(&[]).expect("empty tree");
    let commit = git.write_commit(&tree, &[], "base").expect("base commit");
    git.update_ref("refs/heads/master", commit.object(), None)
        .expect("master");
    (dir, git)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runner's own global configuration, as a shared build machine or a CI image has:
    /// an identity of its own, no identity guessed from the host, and signing
    /// on with a signer that always fails.
    const RUNNERS_CONFIG: &str = "[user]\n\tname = bench\n\temail = bench@localhost\n\tuseConfigOnly = true\n[commit]\n\tgpgSign = true\n[gpg]\n\tprogram = false\n";

    #[test]
    fn a_fixture_commits_as_the_fixture_under_a_runners_own_configuration() {
        let (dir, _git) = repo();
        let home = tempfile::tempdir().expect("runner's home");
        let config = home.path().join(".gitconfig");
        std::fs::write(&config, RUNNERS_CONFIG).expect("runner's config");
        // Git as the code under test starts it: the runner's environment,
        // whose identity comes from its global configuration.
        let as_runner = |args: &[&str]| {
            let mut command = Command::new("git");
            isolated(&mut command)
                .env("HOME", home.path())
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1");
            for name in [
                "GIT_AUTHOR_NAME",
                "GIT_AUTHOR_EMAIL",
                "GIT_COMMITTER_NAME",
                "GIT_COMMITTER_EMAIL",
                "GIT_CONFIG_PARAMETERS",
                "GIT_CONFIG_COUNT",
            ] {
                command.env_remove(name);
            }
            command
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .expect("git")
        };
        let ident = as_runner(&["var", "GIT_COMMITTER_IDENT"]);
        assert!(
            String::from_utf8_lossy(&ident.stdout).starts_with(&format!("{NAME} <{EMAIL}> ")),
            "the repository's own config outranks the runner's: {ident:?}"
        );
        let commit = as_runner(&["commit", "-q", "--allow-empty", "-m", "unsigned"]);
        assert!(commit.status.success(), "never signed: {commit:?}");
        // Git a fixture starts itself reads none of the runner's configuration
        // and none of its identity variables.
        let mut runners = Command::new("git");
        runners
            .env("HOME", home.path())
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_AUTHOR_NAME", "bench")
            .env("GIT_AUTHOR_EMAIL", "bench@localhost");
        let hermetic_ident = hermetic(&mut runners)
            .arg("var")
            .arg("GIT_AUTHOR_IDENT")
            .output()
            .expect("git");
        assert!(
            String::from_utf8_lossy(&hermetic_ident.stdout)
                .starts_with(&format!("{NAME} <{EMAIL}> ")),
            "{hermetic_ident:?}"
        );
    }
}
