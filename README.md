# git-evidence

Numbers recorded per Git commit, kept in Git notes.

A benchmark, a test suite or a CI step records what it measured on a commit: a latency, a byte count, a pass or a fail, or why it could not measure. `git evidence` keeps those records in Git notes beside the code, keyed by the commit's tree, merges them between clones without conflicts, and prints them back as plain rows, or answers SQL over them. It needs nothing but Git: no server, no database to run, no files in the working tree.

From the crate's [quick start](crates/git-evidence/README.md#quick-start), two runs of a benchmark on two commits:

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

Git 2.42 or later is required, and Rust 1.91 or later to build. Installing the binary is enough for Git, which runs any `git-<name>` on `PATH` as `git <name>`; the crate README says how to install the manual pages and how to use it from jj.

## Status

0.x. The event lines (envelope version 2) and the notes layout are permanent: evidence written today stays readable by every later release. The public SQL views change only with their schema version. The command line and the Rust API may change between 0.x minor releases; each change is in the CHANGELOG. [`docs/FORMAT.md`](docs/FORMAT.md#5-what-may-change) has the full policy.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
