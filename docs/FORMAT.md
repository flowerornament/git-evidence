# git-evidence: the format

This is the contract between git-evidence and everything around it: the event lines a producer writes, how they are stored in a Git repository, and the SQL views a reader queries. A producer in any language that follows it can record evidence that `git evidence` reads, and a reader can query evidence without linking the crate.

The key words MUST, MUST NOT, SHOULD and MAY are used as in [RFC 2119](https://www.rfc-editor.org/rfc/rfc2119). How to use the tool is the manual ([`crates/git-evidence/README.md`](../crates/git-evidence/README.md)); why it is built this way is [`DESIGN.md`](DESIGN.md). Where this document and the code disagree, that is a bug in one of them; please report it.

Contents: [1. Event lines](#1-event-lines) · [2. The run grammar](#2-the-run-grammar) · [3. Storage in Git](#3-storage-in-git) · [4. Public SQL views](#4-public-sql-views) · [5. What may change](#5-what-may-change)

## 1. Event lines

A run is a sequence of **event lines**. Each line is one JSON object (RFC 8259), UTF-8, on one line: it MUST NOT contain a line feed. Every line carries the envelope:

| Field   | Type    | Rule                                                                                                  |
| ------- | ------- | ----------------------------------------------------------------------------------------------------- |
| `v`     | integer | the envelope version; MUST be `2`                                                                     |
| `run`   | string  | the run id: a [ULID](https://github.com/ulid/spec), 26 characters of Crockford base32 in upper case (readers accept any 26 such characters) |
| `seq`   | integer | the event's position in its run: `0` for `open`, then `1`, `2`, … without gaps, each used once; at most 2³²−1 |
| `time`  | string  | when the event happened, in UTC, exactly `YYYY-MM-DDTHH:MM:SS.mmmZ` with milliseconds. Producers MUST write a real time; readers check only the shape and the field ranges (month 01–12, day 01–31, hour 00–23, minute 00–59, second 00–60) |
| `event` | string  | `open`, `declare`, `result` or `close`; the event's own fields sit beside the envelope's               |

A run id MUST be unique to its run. A producer SHOULD generate it from the current time and random bits, as a ULID. An importer that projects an existing record MAY derive the id from that record's time and identity, so the same record imported twice names the same run. Runs are ordered by their `open` time, never by id.

Numbers MUST be finite: JSON has no NaN or infinity, and a producer that cannot measure a value records the key `refused` instead (below).

### Names

| Name         | Rule                                                                                                   | Examples                         |
| ------------ | ------------------------------------------------------------------------------------------------------ | -------------------------------- |
| kind         | lowercase `[a-z0-9][a-z0-9._-]*`, at most 64 bytes, no `..`, not ending in `.`                          | `perf`, `tests`, `checks`        |
| producer     | the same rule as a kind                                                                                 | `bench-suite`, `ci-checks`       |
| machine      | 1–64 bytes, no whitespace, `/` or control character; `""` means the run names no machine              | `laptop`, `ci-linux-x86`         |
| key          | 1–1024 bytes, no control character, no leading or trailing whitespace; held exactly as given           | `parser/parse_large`, `a::b::c`  |
| metric       | 1–200 bytes, no whitespace, `/` or control character; MUST NOT start with `verdict.` (reserved)         | `latency.p99_ns`, `rss_bytes`    |
| digest       | `sha256:` and 64 lowercase hex digits                                                                   | `sha256:5d0b…`                   |
| commit       | a full Git object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits                                  |                                  |

A machine is a name the store's owner declares (Git config `evidence.machine`), not necessarily a host name: whatever is recorded is published by `push`.

### `open`

The first event of a run (`seq` 0), written before the producer starts the work, so a crash leaves a visible open run and not a silent gap.

| Field         | Type            | Rule                                                                                                                                         |
| ------------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `commit`      | commit          | REQUIRED. The commit the producer ran on. Its tree is what the run is about and where it is stored.                                          |
| `producer`    | object          | REQUIRED. `name` (a producer name), `version` (string, OPTIONAL).                                                                            |
| `suite`       | object          | OPTIONAL. `harness` (string: the version of what the values mean; bump it when that changes), `params` (object of any JSON values), `metrics` (below). |
| `environment` | object          | OPTIONAL. `machine` (a machine name or `""`), `params` (object of string values: operative settings such as a pinned core), `descriptor` (object of any JSON values: what the machine was, such as CPU and OS). |
| `build`       | object          | OPTIONAL. `profile` (`target`, `flags`: strings, each OPTIONAL), `toolchain` (string), `artifact` (digest of the binary that ran). Each OPTIONAL.        |
| `keys`        | array of keys   | OPTIONAL. The keys the run will answer, when known before the work; else one `declare` names them.                                           |
| `source`      | string          | OPTIONAL. The original record this run imports, by an identity unique to that record (an id, an immutable path, a digest), never by where a copy was found. |

`suite.metrics` maps each metric name to its declaration: `unit` (REQUIRED, a non-blank [UCUM](https://ucum.org/) unit: `ns`, `ms`, `By`, `1`, `%`, `{count}`) and `better` (OPTIONAL, `"lower"` or `"higher"`; absent when neither direction is an improvement).

### `declare`

At most once per run, after `open` and before any `result`, for what the producer learns late.

| Field     | Type          | Rule                                                                                           |
| --------- | ------------- | ---------------------------------------------------------------------------------------------- |
| `keys`    | array of keys | OPTIONAL; only when `open` named none                                                          |
| `metrics` | object        | OPTIONAL; more metric declarations, as in `suite.metrics`                                      |
| `build`   | object        | OPTIONAL; each of `profile`, `toolchain` and `artifact` it carries replaces `open`'s (`profile` as a whole) |

### `result`

One key's outcome.

| Field      | Type   | Rule                                                                         |
| ---------- | ------ | ---------------------------------------------------------------------------- |
| `key`      | key    | REQUIRED; a key the run declared                                             |
| `outcome`  | object | REQUIRED; exactly one of the four outcomes below                             |
| `location` | string | OPTIONAL; the code site that produced it, such as `benches/parser.rs:118`    |

| Outcome                                                  | Means                                                                                                              |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `{"measured": [measure, …]}`                             | values. A measure is `metric` (a declared metric), `value` (number), `n` (OPTIONAL, integer from 0 to 2³²−1: samples) and `mad` (OPTIONAL, number: median absolute deviation, unscaled, in the metric's unit). A measure carries no unit or direction: those are the run's declaration. |
| `{"verdict": {"status": S, "tries": [try, …]}}`          | `S` is `pass`, `fail` or `timeout`. `tries` is OPTIONAL; a try is `status` and, when recorded, `duration_ms` (wall time) and `cpu_ms` (user plus system CPU time of the process that ran the try and the descendants it waited for), both in milliseconds, finite and not negative. An absent time was not recorded; it MUST NOT be written as `0`. |
| `{"refused": {"reason": R, "detail": D}}`                | no value, and retrying may succeed (`noisy_host`, `timeout`, `disk_low`). `detail` OPTIONAL.                       |
| `{"skipped": {"reason": R, "detail": D}}`                | no value, and none is possible at this code (`build_failed`, `compile_error`). `detail` OPTIONAL.                  |

Reasons are the producer's own short codes; git-evidence attaches no meaning to them except `interrupted` by convention (below).

### `close`

The last event of a run.

| Field        | Type   | Rule                                                                                                                                                   |
| ------------ | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `end`        | value  | REQUIRED. `"complete"`: every declared key was answered. `{"refused": {"reason": R, "detail": D}}`: the run stopped, and every unanswered key is refused with that reason. |
| `conditions` | JSON   | OPTIONAL. Free JSON, usually an object: readings taken while the run executed and the producer's own provenance. Only `disturbed` is read (below).                         |

`conditions.disturbed`, a string, says the values were taken under disturbed conditions (load, noise, a drifting machine) and why. The values stand; the `runs` view says `disturbed`. Every other condition is kept as written and exposed, value by value, in the `conditions` view.

A producer that restarts after a crash SHOULD close its own unfinished runs with `"end": {"refused": {"reason": "interrupted"}}`. A run nobody closes stays open and gives no values.

### An example

```json
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":0,"time":"2026-10-08T03:10:02.123Z","event":"open","commit":"9c41e2d0a1b2c3d4e5f60718293a4b5c6d7e8f90","producer":{"name":"bench-suite","version":"1.4.0"},"suite":{"harness":"bench-suite/2","params":{"repeats":20},"metrics":{"latency.p99_ns":{"unit":"ns","better":"lower"}}},"environment":{"machine":"laptop"},"keys":["parse_large","lexer_tokens"]}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":1,"time":"2026-10-08T03:10:09.500Z","event":"result","key":"parse_large","outcome":{"measured":[{"metric":"latency.p99_ns","value":88634,"n":20,"mad":310}]}}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":2,"time":"2026-10-08T03:10:10.250Z","event":"result","key":"lexer_tokens","outcome":{"verdict":{"status":"pass","tries":[{"status":"fail","duration_ms":812},{"status":"pass","duration_ms":790,"cpu_ms":2310}]}}}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":3,"time":"2026-10-08T03:10:11.200Z","event":"close","end":"complete","conditions":{"load":0.4}}
```

## 2. The run grammar

A run's lines, ordered by `seq`, MUST follow

```text
open · declare? · result* · close?
```

and these rules. `git evidence record` (and `Evidence::record` in Rust) refuses a batch that breaks any of them and writes nothing of it; each refusal names the rule and the remedy.

1. `seq` 0 is the `open`; there is one `open` and at most one `declare`, before any `result`.
2. Keys are declared once: in `open.keys` or in `declare.keys`, not both.
3. Each metric is declared once per run with one unit and direction. Declaring it again with the same unit and direction is harmless; with another is a redefinition. A metric whose name starts with `verdict.` MUST NOT be declared: the index derives `verdict.<status>.duration_ms` and `verdict.<status>.cpu_ms` from verdict tries.
4. A `result` answers a declared key, once, and measures only declared metrics.
5. `close` with `"complete"` requires a result for every declared key.
6. Nothing follows `close`.
7. Every number is finite; a try's `duration_ms` and `cpu_ms` are not negative.
8. A run is about one tree: its lines MUST NOT be stored on more than one tree.
9. When the store sets Git config `evidence.<kind>.budget`, a run of that kind MUST NOT take more bytes than that.

A producer MAY deliver a run in several batches (the `open` first, then results as they come, then the `close`). Lines already present are ignored, so delivering a batch twice is harmless.

### How a reader folds a note

A reader groups a note's lines by `run`, orders each run by `seq` and reads it. Readers that are not git-evidence SHOULD fold the same way, so that all readers agree on what a store holds:

- Line order inside a note does not matter, and a line identical in content to another counts once. Blank lines are ignored.
- A line that is not valid JSON or not an event line is reported and skipped.
- A run is read only up to its first missing `seq`: nothing can be added behind a gap, so nothing can be added after a `close`.
- These leave a whole run unreadable (reported, and in no view): two different lines with one `seq`; a line of another envelope version; no `seq` 0, or a `seq` 0 that is not `open` or has a malformed time; a `declare` after a `result`; a metric declared without a unit, redefined, or reserved; a `complete` close with keys unanswered; a machine that is not a machine name; a run id seen on more than one tree (rule 8), wherever it is seen.
- These leave the run readable without the offending line, which is reported: a malformed time after `open`, a second `open`, a second `declare` before any `result`, keys declared twice, a result for an undeclared key, a second result for a key, a measure of an undeclared metric, anything after `close`.
- A line skipped for a malformed time, or for following `close`, is not otherwise read: it neither breaks nor satisfies another rule (a `result` with a malformed time does not count as a result for the `declare` rule).
- A run that never closed is open: it gives no values.
- An ended run's unanswered declared keys are refused with the `close`'s reason.

Readers MUST ignore fields they do not know (section 5).

## 3. Storage in Git

Evidence lives in Git notes, one notes ref per kind, keyed by tree:

```text
refs/notes/evidence/<kind>                     the store's evidence of one kind; what fetch merges into and push sends
refs/notes/remotes/<remote>/evidence/<kind>    the last fetched copy of a remote's ref; never written by producers
refs/notes/evidence-staging/<kind>             transient, used by a multi-tree record; never read, fetched or pushed
```

- **The note's object is the tree.** A run's lines are stored in the note attached to the tree object of its `open.commit` (`git rev-parse <commit>^{tree}`), not to the commit. Code that lands unchanged has the same tree, so it carries its evidence with it.
- **A note is lines.** The note's blob is event lines, each terminated by a line feed (`\n`), in any order. Every run's lines on that tree, of that kind, share the one note.
- **Appending.** A writer appends lines to the note without a separator or whitespace cleanup (`git notes --ref evidence/<kind> append --no-separator --no-stripspace -F - <tree>`). Writers within one repository serialise; git-evidence takes a lock file per notes ref under `<git-common-dir>/evidence/locks/`. A writer that does not use git-evidence MUST NOT run concurrently with one that does.
- **Merging.** Two stores' refs merge by set union of lines: `git notes merge -s cat_sort_uniq`. Lines are immutable and unique, so a merge never conflicts and never loses a line.
- **Grow-only.** No line is ever removed or rewritten in normal operation. History is rewritten only to remove a leak, by hand.
- **The tree's commit.** Runs are placed on the trunk by tree: a run's `landing` is the first commit on the trunk's first-parent line whose tree is the run's tree. A run whose `commit` is itself a landing but whose note sits on another tree is misfiled; it is reported and in no view.

Recording through `git evidence record KIND` is the supported way to write: it checks the grammar of the whole batch against what is already stored, keeps a batch over several trees all-or-nothing (on the staging ref, then one compare-and-swap of the kind's ref), and returns only once the objects, the ref and its directory are on disk. A producer that writes notes directly takes on these duties itself.

The store's Git config holds the settings: `evidence.machine`, `evidence.<kind>.budget` and `evidence.push` are read from the repository's own config (`git config --local`) only, `evidence.remote` from any level. No file in the working tree is read.

The index (`<git-common-dir>/evidence/index-v<views>-<source>.sqlite`) is a cache derived from the refs above. It is not part of the format: it MAY be deleted at any time, and its file name and private tables change between builds.

## 4. Public SQL views

`git evidence sql QUERY` runs one read-only query on the index; `git evidence sql --path` prints the index file for another SQLite reader. The **public views** below are the contract; every other table in the file is private. The views are **schema 6**: `SELECT schema_version FROM about`, and the file's `PRAGMA user_version`.

| View         | One row per                | Columns (type)                                                                                                                                                                       |
| ------------ | -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `about`      | the index                  | `schema_version` INT, `views_version` INT, `trunk_ref` TEXT, `trunk` TEXT (the trunk commit read; NULL when the ref does not resolve), `public_views` TEXT, `pending_views` TEXT     |
| `landings`   | commit on the trunk's first-parent line | `landing` INTEGER (0 is the oldest), `commit_id` TEXT, `tree` TEXT, `date` TEXT (committer time, `YYYY-MM-DDTHH:MM:SSZ`), `subject` TEXT                                 |
| `runs`       | run                        | `run` TEXT, `kind` TEXT, `producer` TEXT, `machine` TEXT (NULL when none), `variant` TEXT, `conditions` TEXT (`disturbed` or `quiet`), `disturbed` TEXT (its reason), `landing` INT (NULL off the trunk), `tree` TEXT, `commit_id` TEXT, `opened` TEXT, `closed` TEXT (NULL while open), `ended` TEXT (`complete`, `refused`, NULL while open), `reason` TEXT |
| `params`     | run parameter              | `run` TEXT, `name` TEXT, `value` TEXT (JSON text when not a string). Suite and environment parameters.                                                                               |
| `conditions` | value a run closed with    | `run` TEXT, `path` TEXT (dotted: `load`, `findings.0.id`), `value` TEXT (JSON text when not a string; NULL for `null`)                                                               |
| `measures`   | value                      | `run` TEXT, `key` TEXT, `metric` TEXT, `value` REAL, `unit` TEXT, `direction` TEXT (`lower`, `higher`, NULL), `n` INTEGER, `mad` REAL, `origin` TEXT (`recorded`, `derived`)          |
| `verdicts`   | verdict                    | `run` TEXT, `key` TEXT, `status` TEXT, `duration_ms` REAL, `cpu_ms` REAL (sums over the tries; NULL unless every try has one), `tries` INTEGER                                      |
| `outcomes`   | key a run answered         | `run` TEXT, `key` TEXT, `outcome` TEXT (`measured`, `verdict`, `refused`, `skipped`), `status` TEXT, `reason` TEXT, `detail` TEXT, `location` TEXT                                 |

- Only readable runs appear (section 2); an open run appears in `runs` with `closed` and `ended` NULL and gives no measures.
- `variant` is a digest of what must match for two values to be compared: kind, producer, harness, suite params, build profile, machine and environment params. It is derived by the index under its views version; producers never compute it.
- `measures` with `origin = 'derived'` are the verdict times `verdict.<status>.duration_ms` and `verdict.<status>.cpu_ms` (unit `ms`, lower is better), present only when every try of the verdict has that time.
- Times are text in the forms above and sort as text. Booleans are 0 or 1.

The column list of every schema version is pinned, one section per version, in [`crates/git-evidence/tests/public_schema.snap`](../crates/git-evidence/tests/public_schema.snap); a test fails if the views drift from it.

## 5. What may change

git-evidence is 0.x. Evidence pushed to a remote lives in every clone, so the parts of this document that describe stored data are held to a stricter rule than the code:

| Part                                         | Rule                                                                                                                                                                         |
| -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Event lines, envelope `v: 2` (sections 1–2)  | Permanent. Any release reads every line any earlier release accepted. New OPTIONAL fields MAY be added in a minor release; readers ignore fields they do not know. A change that would alter the meaning of an existing line is a new envelope version, and readers keep reading version 2. |
| Notes layout (section 3)                     | Permanent: the refs, keying by tree, the line format and the union merge.                                                                                                    |
| Public SQL views (section 4)                 | A view's name, columns and their types change only with a new schema version, in a minor release, recorded in the CHANGELOG with its old and new columns.                    |
| The index file, its name and private tables  | Not a contract; may change in any release. A new build rebuilds its index from the notes.                                                                                    |
| CLI output columns, flags and the Rust API   | Documented in the manual; may change between 0.x minor releases, each change recorded in the CHANGELOG.                                                                      |

Producers SHOULD NOT add fields of their own to event lines: a future release may give a name a meaning. A producer's own data belongs where the format leaves room for it: `suite.params`, `environment.params`, `environment.descriptor` and `close.conditions`.
