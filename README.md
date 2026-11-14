# git-evidence

git-evidence lets you store numbers, like performance measurements, on any Git commit.

You record what a benchmark, a test suite or a CI step measured on a commit: a time, a size, a pass or a fail. When one of them could not measure, you record why, so a missing number never looks like a good one.

Your records are kept in Git notes inside the repository. You share them with `git evidence push` and pick up other people's with `git evidence fetch`, and records from different clones merge without conflicts. Because records are filed under the commit's tree, numbers you measure on a branch still apply when that exact code lands.

You can read the numbers with the built-in commands (`show`, `trend`, `diff`), which print plain rows. For anything else, git-evidence builds a SQLite database from the notes, and you, a script or a coding agent can query it with SQL.

You can use it to track a program's performance across commits, or to record how long each CI step takes so you can make your development loop faster, with evidence.

[`docs/USAGE.md`](docs/USAGE.md) shows a full setup, and explains why the numbers live in Git notes and not in a file.

## Example

From the crate's [quick start](crates/git-evidence/README.md#quick-start), two runs of a benchmark on two commits. A landing is a commit on the trunk (the main branch, by first parent); the `landing` column numbers them from the oldest, `0` first:

```console
$ git evidence trend latency.p99_ns parse_large
landing  commit        date                  machine  value  unit  run
      0  2f263a8ba34b  2026-10-08T03:00:00Z  laptop   88634  ns    01JA8Z3K4M0000000000000000
      1  11730712b4f6  2026-10-08T03:00:00Z  laptop   80412  ns    01JA8Z3K4M0000000000000001
$ git evidence diff HEAD~1 HEAD
kind  machine  variant   key          metric          before  after  change  unit  before_runs  after_runs  before_run                  after_run
perf  laptop   ecd56f15  parse_large  latency.p99_ns   88634  80412   -9.3%  ns              1           1  01JA8Z3K4M0000000000000000  01JA8Z3K4M0000000000000001
perf  laptop   ecd56f15  parse_small  latency.p99_ns    1210                 ns              1              01JA8Z3K4M0000000000000000
```

## This repository

| Crate                                 | What it is                                                                                                                                            | Manual                                                                                                                            |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| [`git-evidence`](crates/git-evidence) | the `git evidence` command and its Rust library: record runs, fetch and push them, ask `show`, `log`, `trend`, `diff`, `keys` and `sql`               | [README](crates/git-evidence/README.md) · [CHANGELOG](crates/git-evidence/CHANGELOG.md) · [docs.rs](https://docs.rs/git-evidence) |
| [`git-process`](crates/git-process)   | a hardened `git` subprocess for Rust: one invocation prefix, typed object ids, retried starts. git-evidence's only way to reach Git, and usable alone | [README](crates/git-process/README.md) · [CHANGELOG](crates/git-process/CHANGELOG.md) · [docs.rs](https://docs.rs/git-process)    |

Both are released together, at one version.

| Document                                                         | For                                                                                                                     |
| ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| [`crates/git-evidence/README.md`](crates/git-evidence/README.md) | using the tool: quick start, every verb, column and setting                                                             |
| [`docs/USAGE.md`](docs/USAGE.md) | the problem it solves, a project's setup end to end, and why it is shaped this way (and not a file in the repository) |
| [`docs/FORMAT.md`](docs/FORMAT.md)                               | writing a producer or reader in another language: the event lines, the notes layout, the SQL views, and what may change |
| [`docs/DESIGN.md`](docs/DESIGN.md)                               | why it is built this way: the laws, the write path, the index                                                           |
| [`CONTRIBUTING.md`](CONTRIBUTING.md)                             | sending a change                                                                                                        |
| [`SECURITY.md`](SECURITY.md)                                     | reporting a vulnerability or a leak of private data                                                                     |
| [`docs/RELEASE.md`](docs/RELEASE.md)                             | cutting a release                                                                                                       |

## Install

```sh
cargo install git-evidence
git evidence -h
```

Git 2.42 or later is required (the crate README's Install says why), and Rust 1.91 or later to build. Installing the binary is enough for Git, which runs any `git-<name>` on `PATH` as `git <name>`; the crate README says how to install the manual pages and how to use it from jj.

## Status

0.x. The event lines (envelope version 2) and the notes layout are permanent: evidence written today stays readable by every later release. The public SQL views change only with their schema version. The command line and the Rust API may change between 0.x minor releases; each change is in the CHANGELOG. [`docs/FORMAT.md`](docs/FORMAT.md#5-what-may-change) has the full policy.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
