# Releasing

`git-process` and `git-evidence` are released together, at one version, from a tag `vX.Y.Z` on `main`. The tag is the only manual publication act: the **Release** workflow (`.github/workflows/release.yml`) checks that the tagged commit is on `main`, runs `scripts/release.sh verify` (the version, the changelogs, the gate and a publish dry run), publishes both crates to crates.io and creates the GitHub release with the changelogs' sections as its notes.

## The version

`[workspace.package] version` in the root `Cargo.toml` is the one version. Both crates inherit it (`version.workspace = true`), and git-evidence depends on git-process through `[workspace.dependencies]`, whose `version` must name the same release. `Cargo.lock` and `rust-toolchain.toml` pin the dependencies and the compiler; the Nix package reads the version from `Cargo.toml`.

While the version is 0.x, a minor release (0.2.0) may change the command line and the Rust API, and a patch release (0.1.1) fixes without changing either. The event lines and the notes layout never break, and the public SQL views change only with a new schema version ([`FORMAT.md`](FORMAT.md#5-what-may-change)). A change to git-process's API that git-evidence re-exports (object ids, `IdError`, `Timestamp`) is a change to git-evidence's API too.

## One-time setup

- A crates.io API token with the `publish-update` scope for both crates (and `publish-new` for the first release), stored as the repository secret `CARGO_REGISTRY_TOKEN`. The release job runs in the GitHub environment `crates-io`; restrict the secret and the environment to tags `v*` there.
- After the first release, crates.io trusted publishing can replace the token.

## Cutting a release

1. In a change of its own, set the version in `Cargo.toml` (`[workspace.package] version` and the `git-process` entry in `[workspace.dependencies]`), run `cargo update --workspace` so `Cargo.lock` agrees, and in each crate's `CHANGELOG.md` rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` (today, UTC), adding a fresh empty `## [Unreleased]` above it. A crate with nothing to record says so in its section ("Released with git-evidence X.Y.Z; no changes."). Describe it `release: X.Y.Z (<goal-id>)` and land it with `just land`.
2. Wait for **CI** to pass on that commit.
3. `just release-verify X.Y.Z` on the landed commit: the workspace version and both crates are X.Y.Z, both changelogs have a dated, filled section, `just check` passes, and `cargo publish --workspace --dry-run` packages and builds both crates.
4. Tag the landed commit and push the tag, from the colocated checkout:

   ```sh
   git tag -a vX.Y.Z -m "git-evidence X.Y.Z" <commit>
   git push origin vX.Y.Z
   ```

5. Watch the **Release** workflow. When it is green, `cargo install git-evidence --version X.Y.Z` installs the release, and docs.rs builds both crates' documentation within the hour.

A published crate version cannot be replaced, only yanked (`cargo yank --version X.Y.Z <crate>`), so a mistake found after publishing is fixed by a new patch release.

## When the workflow fails

- **Before publishing** (tag check, notes, gate): nothing was published. Delete the tag (`git push origin :refs/tags/vX.Y.Z` and `git tag -d vX.Y.Z`), land the fix, and tag again.
- **After `git-process` was published, before `git-evidence`:** do not delete the tag. Fix the cause (a token, a network failure) and re-run only what is left from a checkout of the tag: `cargo publish --locked --package git-evidence`, then `gh release create vX.Y.Z --verify-tag --notes-file <(scripts/release.sh notes X.Y.Z)`. If git-evidence itself needs a code change, release X.Y.Z+1 of both crates.
- **Only the GitHub release failed:** run the `gh release create` line above.

## Installing

- From crates.io: `cargo install git-evidence`, then install the manual pages as the crate README's "Revisions, jj and the manual" says.
- With Nix: the flake's `packages.<system>.default` installs `bin/git-evidence`, wrapped to find Git, with its manual pages in `share/man/man1`. Pin a release by tag (`github:flowerornament/git-evidence/vX.Y.Z`) or a commit (`?rev=<commit>`).
- As a library: `cargo add git-evidence --no-default-features`, which leaves out the binary and its dependencies (feature `cli`), or `cargo add git-process` alone.
