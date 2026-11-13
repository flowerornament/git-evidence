# Changelog

All notable changes to `git-evidence` are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html): while it is 0.x, the event lines and the notes layout never break, the public SQL views change only with their schema version, and the CLI and the Rust API may change between minors ([`docs/FORMAT.md`](https://github.com/flowerornament/git-evidence/blob/main/docs/FORMAT.md#5-what-may-change)).

## [Unreleased]

## [0.1.0] - 2026-11-14

The first release, 0.1.0. `git-process` is released with it at the same version.

### Added

- **Event lines, version 2.** A run is `open · declare? · result* · close?`: opened before the work, each metric declared once with a UCUM unit and an optional direction, an optional `declare` before any result, one result per declared key with one of four outcomes (`measured`, `verdict` with its tries and their wall and CPU times, `refused`, `skipped`), and a close that is `complete` or `refused` with free-JSON conditions. The normative contract for producers is [`docs/FORMAT.md`](https://github.com/flowerornament/git-evidence/blob/main/docs/FORMAT.md).
- **Storage in Git notes**, keyed by tree, one ref per kind (`refs/notes/evidence/<kind>`). `record` checks a whole batch against the run grammar and what is stored, writes it all or nothing across trees, and returns once the objects, the ref and its directory are on disk. Per-kind byte budgets (`evidence.<kind>.budget`).
- **Sharing.** `fetch` union-merges a remote's evidence (`git notes merge -s cat_sort_uniq`); `audit` reports sizes, unreadable runs and text that looks personal; `push` is opt-in (`evidence.push`), audits everything it sends, and merges and retries when another store pushed first.
- **Reading.** `git evidence` (the newest landings and their run counts), `show REV|RUN`, `log`, `trend METRIC KEY`, `diff A B` and `keys`, printing a header and one row per fact: aligned columns at a terminal, CSV with `--csv`. Landings are selected by `A..B`, `--since`, `--until` and `--last N`. Exit status 0 printed, 1 could not run, 2 nothing recorded for what was named, and also a refused `record`, `fetch` or `push` and an `audit` with findings.
- **SQL.** `git evidence sql QUERY` runs one read-only query on the refreshed index; `sql --path` prints the index file. Public views, schema 6: `about`, `landings`, `runs`, `params`, `conditions`, `measures`, `verdicts`, `outcomes`.
- **The index**, a disposable SQLite cache under `<git-common-dir>/evidence/`, one file per build, caught up incrementally from the notes, with every read under a time limit (`--time-limit`, default 30 s).
- **Revisions in Git and jj workspaces**: in a jj workspace a revision is a jj revset first, and `HEAD` means `@-`. Shallow and partial clones are refused.
- **`git evidence bench`**, the tool's own benchmark, on a generated store or a copy of this one; it can return its figures as a run of kind `tool`.
- **The manual**, rendered from the command line: `git-evidence manual DIR` writes `git-evidence.1` and one page per verb.
- **The Rust API**: `Evidence` (`run`, `resume`, `record`, `at`, `sql`) and `Run::finish`, a query type per verb, `TidyRows` for every answer, typed ids, `OwnNames` for the audit's judgement of outside text, and `testing::Scratch` (feature `testing`) for an integration's tests. Types that may grow (the format's structs, answers, queries and errors) are `#[non_exhaustive]` and built through constructors (`Open::new` with `harness` and `machine`, `Outcome::measured`, `Verdict::new`, `Try::new`, `Metric::lower`, `Declaration::keys`, `EventLine::new`, `Event::result`, `Event::close`, a query's `new` and one builder method per filter). The `bench` feature, on by default, carries the bench verb's types.
- **Guarantees**, each enforced by named tests: grow-only, idempotent, order-free, ended runs answer every declared key, one run per tree, reproducible, matched within a variant, durable before return (crash tests by real process aborts, with the `fault-injection` feature).
