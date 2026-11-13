# git-evidence

Numbers recorded per Git commit, kept in Git notes (`crates/git-evidence`), and the hardened `git` subprocess it runs on (`crates/git-process`). Both are public open-source crates: everything in this repository, commit messages included, is written for a reader who has never seen the projects that use them.

## Goals (bd)

bd holds **goals**, not tasks. These rules override bd's generic guidance, including what `bd prime` prints.

- **A goal** has a why, a done-when that can fail, and an appetite (how much time or effort it is worth).
- **The how** is the owner's choice. Never file it, or a plan step, as a bd item.
- **Findings go on the goal** as comments. When the goal closes, its owner promotes each finding to a new goal or drops it. Never file anything "for follow-up".
- **Appetite spent, goal not done:** land what holds, narrow the goal within its why, or park it with a finding. Never extend it silently.
- **Every landing** (a change published to the main branch) cites a goal in its subject: `area: subject (<goal-id>)`.
- **One owner per goal**, claimed atomically. One goal in progress per agent.
- **Memory:** do not keep it in bd (`bd remember`) or in ad hoc files. Put durable facts in the code, the living documents or a dated design document.

```bash
# orient
bd show --current --short     # what is in progress
bd ready --explain            # goals with no blockers

# work
bd update <id> --claim        # claim atomically (one owner)
bd comments add <id> "..."    # a finding, on the goal
bd close <id>                 # done-when holds

# write a goal (why and appetite in the description)
bd create --title="..." --type=feature|bug --priority=2 \
  --acceptance "Done when: <a check that can fail>"

# query
bd query "status=in_progress"
bd search "keyword"
bd graph --compact <id>
```

A bug is a goal whose done-when is its repro passing. Full reference: `bd prime` (the rules above override its filing advice).

## Build & Quality

A Rust workspace of two crates, released together at one version. Run every build operation through `just`.

```bash
just check            # the gate: fmt, strict clippy, every test with the test-only features, API docs, both crates packaged
just package          # package both crates and build each from its package
just fmt              # cargo fmt
just lint             # clippy, warnings denied
just test             # cargo test, all features
just build            # release build
just release-verify X.Y.Z   # every check of a release short of publishing (docs/RELEASE.md)
```

- `rust-toolchain.toml` pins the toolchain; do not run rustup by hand.
- Add dependencies with `cargo add`; never write a version string from memory.
- Lints are the workspace's (`[workspace.lints]` in `Cargo.toml`); an exception states its reason (`#[expect(lint, reason = "…")]`).
- Public tree and history carry no private data: no personal names, host names, home paths or internal identifiers of other projects. Test fixtures use made-up names. Private operating notes go in `AGENTS.local.md` (ignored).

## Project rules

- **Documents.** Living documents are kept true to the code, in the same change as the behaviour they describe: the root `README.md` (the front door), each crate's `README.md` (the manual; its quick start and SQL examples run as tests) and `CHANGELOG.md` (under `[Unreleased]`), `docs/FORMAT.md` (the normative contract for producers and readers), `docs/DESIGN.md` (what holds and why), `docs/USAGE.md` (the problem and a project's setup), `docs/RELEASE.md`, `CONTRIBUTING.md`, `SECURITY.md` and this file. A path or name a document cites resolves in the tree.
- **Permanent formats.** Pushed evidence lives in every clone. Never change the meaning of an event line (envelope `v: 2`) or the notes layout; a new optional field is the most a release may add (`docs/FORMAT.md`, section 5). A change to a public SQL view's name, columns or types is a new schema version and a new section appended to `crates/git-evidence/tests/public_schema.snap`, never an edit of an old one.
- **No verdicts.** The tool records and prints numbers; judging them is the user's query. A proposal to judge (regressions, thresholds, expectations) is out of scope (`docs/DESIGN.md`, "Left out on purpose").
- **One version.** Both crates inherit `[workspace.package] version` and are released together from a tag (`docs/RELEASE.md`); git-process is published first. Never tag, publish or push a tag without the maintainer's go.
- **The public API is pinned.** `crates/git-evidence/tests/public-surface.txt` lists the crate root's exported names; a change to it is an API change and goes in the CHANGELOG.
- **Nix in a workspace.** A jj workspace has no `.git`, so a flake reference `.` would be read from whatever repository encloses it. Use `nix build path:.` and `nix develop path:.` in a workspace; CI and the anchor can use `.`.

## Version Control

This project uses jj, colocated with git. All agents share one commit graph. jj runs with stock configuration and records edits continuously, so there is nothing to stage.

Terms (the `jj-ops` skill uses the same ones):

- **trunk**: the one published bookmark, `main`.
- **anchor**: the colocated checkout. It holds `.git`, `.jj`, the bd home and releases. No agent edits code there.
- **seat**: a standing jj workspace with a numbered name. Each writing agent works in its own seat. In a pair, `a` is the coordinator and `b` the implementer.
- **gate**: `just check`.
- **land**: publish a described change with `just land`.
- **freeze**: stop editing a change and hand its commit id to a reviewer.

```text
git-evidence/        anchor (the colocated checkout)
git-evidence-<n>a/   seat, coordinator
git-evidence-<n>b/   seat, implementer
```

- Work: `jj new main`, edit, then `jj describe -m "area: subject (goal-id)"`.
- Pick up others' landings with `jj rebase -d main`.
- Land with `just land`. It runs the gate on exactly the described change, moves the trunk bookmark to that change, and pushes.
- `just land` refuses undescribed work, a stale parent, and any edit made while the gate ran.
- The implementer lands only after the coordinator's GO (approval to land).
- Never run a raw `jj git push`; it skips the gate.
- `scripts/jj-identity`, run by a SessionStart hook, adds `Jj-Workspace:`, `Agent:` and `Session:` trailers to every description. Do not type or strip them.
- Run git only in the anchor, and only commands that read. A seat has no `.git`, so git in a seat may silently report on an enclosing repository. Never run a git command that writes.
- Review a frozen commit in one visit, by commit id: `jj new <commit-id>`; when done, `jj abandon @` and `jj new main`. Each move of `@` costs a rebuild.
- When another agent rewrites a commit under your `@`, your `@` is stale: every jj command refuses until you run `jj workspace update-stale`.
- Never `jj edit` a change that is an ancestor of another seat's `@`.
- To recover, start with `jj op log`, `jj undo` and `jj op restore`. Use them before any destructive file operation. The `jj-ops` skill holds the jj model.

## Completion

1. `jj describe -m "area: subject (goal-id)"`, then `just land`.
2. `bd dolt push` if goals changed.
3. Work is not complete until `just land` succeeds.
