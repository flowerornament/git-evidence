# Changelog

All notable changes to `git-process` are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first version, to be released as 0.1.0, together with `git-evidence` at the same version: `git-evidence` re-exports its object ids, `IdError` and `Timestamp`, so the two are versioned and released together. 0.x because its API may still change between minors.

### Added

- `Git`: one invocation prefix for every `git` command (no automatic gc, objects and refs flushed to disk before a command returns, a fixed locale, no prompts), and `isolated`, which removes the inherited variables that redirect Git.
- Typed object ids, validated where they are parsed.
- `Timestamp`: a UTC time to the millisecond in its one wire spelling, `YYYY-MM-DDTHH:MM:SS.mmmZ`, parsed once where it is read; `StartFailureRecord::time` is one.
- `start()`: starts a process, retrying a start the OS refused for lack of resources, and reports each failed attempt to an optional observer (`StartObserver`, installed once by `observe_start_failures`).
- `GitError` and `StartFailureRecord` are `#[non_exhaustive]`, so a new error or a new field is not a breaking change.
- `testing::repo` (feature `testing`): a throwaway repository with one commit, for tests of Git readers.
