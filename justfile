# Against the committed lock file; CI runs exactly this on Linux and macOS.
#
# The gate: fmt, strict clippy (also on the libraries as a caller with
# `default-features = false` builds them, with and without `testing`), every
# test and doc test, API docs, both crates packaged
check: && package
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo clippy --workspace --lib --no-default-features --locked -- -D warnings
    cargo clippy -p git-evidence --lib --no-default-features --features testing --locked -- -D warnings
    cargo test --workspace --all-features --locked
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --locked

# git-evidence's copy resolves against the packaged git-process. The check is
# about contents, not version control, so a working copy with edits (a jj
# workspace, an unlanded change) packages as it stands.
#
# Package both crates and build each from its package, git-process first
package:
    cargo package --workspace --locked --allow-dirty

# Every check of a release of VERSION short of publishing (docs/RELEASE.md)
release-verify version:
    scripts/release.sh verify {{quote(version)}}

# Publish the described jj change: run `just check` on exactly it, then push it
land *args:
    scripts/jj-land.sh {{args}}

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
    cargo test --workspace --all-features --locked

build:
    cargo build --release --locked
