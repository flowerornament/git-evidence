# Contributing

Thanks for your interest. git-evidence is small on purpose: it records numbers per commit and prints them back, and leaves judging them (what counts as a regression, which runs to trust) to the projects that use it. A change that keeps it that way, that makes a record more honest or that deletes code is the easiest kind to accept. Read [`docs/DESIGN.md`](docs/DESIGN.md) before proposing a new mechanism, and especially its "Left out on purpose".

## Reporting a problem

Open a GitHub issue with:

- `git evidence --version` and `git --version`, and your operating system;
- the command you ran, what you expected and what happened, with standard error;
- if it concerns stored evidence, the event lines involved (`git notes --ref evidence/<kind> show <tree>`), with anything private removed.

A security problem, or private data that `audit` let through, goes to [`SECURITY.md`](SECURITY.md) instead, not to a public issue.

## Making a change

1. Enter the dev shell: `nix develop` gives the pinned Rust toolchain, Git, [jj](https://jj-vcs.github.io/jj/) (some tests drive jj workspaces) and `just`. Without Nix, install the toolchain in `rust-toolchain.toml`, Git 2.42 or later, jj and `just`.
2. Make the change with a test that fails without it. A behaviour of the command line is tested through the binary (`crates/git-evidence/tests/`); a new write path that can fail half-way needs a fault point (feature `fault-injection`) and a crash test.
3. Update the living documents in the same change:
   - the crate README for anything a user sees (it is the manual; its quick start and SQL examples run as tests);
   - [`docs/FORMAT.md`](docs/FORMAT.md) for anything about event lines, the notes layout or the public views. The event lines and the notes layout are permanent; a change to a public view needs a new schema version and a new section in `crates/git-evidence/tests/public_schema.snap`;
   - [`docs/DESIGN.md`](docs/DESIGN.md) if a law or a mechanism changes;
   - the crate's `CHANGELOG.md`, under `[Unreleased]`.
4. Run `just check`: rustfmt, Clippy with the workspace lints denied, every test with the `testing` and `fault-injection` features, and `cargo package` of both crates. CI runs the same on Linux and macOS.
5. Open a pull request that says what the change adds or deletes, and which documents it updates.

The workspace lints are strict (Clippy `pedantic` denied, no `unwrap`, no `print!` in the library). An `#[allow]` needs a `reason`.

[`AGENTS.md`](AGENTS.md) is the working agreement the maintainers and their coding agents follow. You don't need it to send a pull request.

## License

By contributing, you agree that your contribution is dual licensed under the [MIT license](LICENSE-MIT) and the [Apache License, Version 2.0](LICENSE-APACHE), as described in the [README](README.md#license).
