
# git-evidence: the design

git-evidence keeps what a producer measured on a commit (a time, a byte count, a pass or a fail, or why it could not measure) in Git notes beside the code, merges stores without conflicts, and prints the numbers back as rows. It is a standalone, project-agnostic crate: `crates/git-evidence/` depends on Git, through `crates/git-process/`, and on no project that uses it.

This doc says what holds and why. How to use the tool, every flag and every column, is the manual: [`crates/git-evidence/README.md`](../crates/git-evidence/README.md), with [`crates/git-evidence/CHANGELOG.md`](../crates/git-evidence/CHANGELOG.md) for what changed. The bytes a producer writes and the views a reader queries are specified normatively in [`FORMAT.md`](FORMAT.md). Where this doc and the code disagree, the code is right and this doc is fixed in the same change.

```text
producer ──record──► refs/notes/evidence/<kind>  ──fold changed notes──► index-v<views>-<source>.sqlite
 (benchmark, CI,      one note per tree:              (a cache; delete it freely)
  test suite, script) every run's event lines                      │
                            ▲  │                                   ▼
                     fetch  │  │ push (after audit)      rows ─┬─ aligned columns
                            │  ▼                               ├─ --csv
                        another store                          └─ sql over public views (schema 6)
```

## 1. The data model

The nouns (tree, kind, run, key, metric, outcome, machine, variant, landing) are defined once, in the manual's ["The model"](../crates/git-evidence/README.md#the-model). The design choices behind them:

Nothing else has an identity: no writer names, store ids or method hashes. Producers never hash; the variant is derived by the index, under the views version, so a mistake in what it includes is fixed by a version bump, not by rewriting evidence. Toolchain, artifact and machine descriptor are left out of the variant on purpose: they are what a comparison measures, or would split a series on a kernel update.

**A run is event lines** in the grammar `open · declare? · result* · close?`. Every line carries `v` (2), `run`, `seq` (0, 1, … without gaps) and `time` (`YYYY-MM-DDTHH:MM:SS.mmmZ`, the one form that sorts as text). `open` names the commit, producer, suite (harness, params, metrics) and environment; `declare` may add keys and metrics once, before any result (a later one leaves the run unreadable); each `result` answers one declared key; `close` ends `complete` (every declared key answered) or `refused{reason}` (every unanswered key refused), with free-JSON `conditions`. A producer opens before it works, so a crash leaves a visible open run, never a gap; an open run gives no values.

**Why notes keyed by tree.** ([`USAGE.md`](USAGE.md) has the longer case, including why not a file in the repository.) Evidence is about code, and a tree is the code. Keying by tree lets identical code share evidence across commits and survive a rebase that changes nothing; Git's fanout keeps an append the same cost at any size. Notes are not branches, so no checkout, bookmark or jj operation ever moves them.

## 2. Laws

Where each law is tested is listed once, in the manual's ["Guarantees"](../crates/git-evidence/README.md#guarantees).

| Law                                                                                                          | Holds because                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Grow-only.** A ref's lines only accumulate, and merging two stores gives exactly the union of their lines. | No verb rewrites or removes a line; `close` is last and a run is read only up to its first missing `seq`, so nothing can be added behind it. Lines are immutable and unique, so a note is a set; `fetch` merges with `git notes merge -s cat_sort_uniq`, which is set union. Nothing conflicts, nothing is forced. |
| **Idempotent.** A line or batch delivered twice changes nothing.                                             | Lines already present are ignored; a crashed `record` or `push` is simply run again.                                                                                                                                                                                                                               |
| **Order-free.** Any line order folds to the same runs.                                                       | The fold groups by run and orders by `seq`; the fold is the one place the grammar is read.                                                                                                                                                                                                                         |
| **Ended runs answer every declared key.**                                                                    | A `complete` close with keys unanswered is refused at write and unreadable at read.                                                                                                                                                                                                                                |
| **One run, one tree.**                                                                                       | A run id seen on two trees is refused where `record` can see it and excluded from every row where it cannot (`sightings`).                                                                                                                                                                                         |
| **Reproducible.** Same notes and trunk give the same rows; catching up equals a fresh rebuild.               | Nothing reads the clock; the index is a function of the refs' tips.                                                                                                                                                                                                                                                |
| **Matched within a variant.** `diff` never sets one variant's number beside another's.                       | Rows match on kind, machine, variant, key, metric and unit; units are never converted.                                                                                                                                                                                                                             |
| **Durable before return.**                                                                                   | `record` returns after the objects, the ref and the ref's directory are synced (`F_FULLFSYNC` on macOS).                                                                                                                                                                                                           |

## 3. Recording

```text
lines ─► check each line ─► find each run's tree ─► take kind lock ─► plan whole batch ─► append ─► sync ─► rows
         (version, time,     (open's commit, or       (one per ref)     (grammar, metrics,   one tree: on the ref
          finite numbers)     where it is recorded)                      budget; refuse all)  several: staging ref + CAS
```

- **One write path.** `git evidence record KIND` and `Evidence::record` are the same call (`crates/git-evidence/src/write/batch.rs`). A batch is any lines of one kind: a whole run, or the next events of a streamed one. It is all-or-nothing across trees: one tree appends on the kind's ref directly (Git moves it atomically); several are staged on `refs/notes/evidence-staging/<kind>` and the kind's ref moves once by compare-and-swap (`crates/git-evidence/src/store.rs`).
- **Streaming.** `Evidence::run` opens a `Run` before the work and writes event by event, to this store or to a file another store records (`crates/git-evidence/src/write/run.rs`, `crates/git-evidence/src/write/destination.rs`). `Finish` writes the rest (declaration, results, close) as one commit (`crates/git-evidence/src/write/finish.rs`). `Evidence::resume` finds a run after a crash.
- **Refusals.** A refused batch writes nothing and names its rule and remedy: `Refusal` (`crates/git-evidence/src/write/refusal.rs`): another envelope version, a time in another form, a non-finite number, a negative wall or CPU time, an unknown commit, a run on another tree, events with no open, resuming an unrecorded or ended run, over budget, and the grammar and metric rules (`Problem`: undeclared or redefined metric, a declare after a result, a declared `verdict.` metric, a second result, anything after close, a `seq` gap, a bad machine name, …).
- **Metrics are declared once.** A measure carries only metric, value, `n` and `mad`; its unit and direction are the run's declaration, so a unit is never guessed. `verdict.<status>.duration_ms` and `.cpu_ms` are reserved: the index derives them from verdict tries (`origin = derived`).
- **Budgets.** `evidence.<kind>.budget` caps one run's bytes; unset is no budget, and an unreadable value is an error, never "no budget" (`Store::budget` in `crates/git-evidence/src/store.rs`).
- **Machine.** Recording through the Rust API needs `evidence.machine`; the store, not the producer, says where it runs.

## 4. Sharing

`fetch`, `push` and `audit` live in `crates/git-evidence/src/share/`; their flags and behaviour are the README's "Sharing". Git and jj move neither notes nor custom refs by default, so evidence travels only through these verbs. `fetch` merges by set union, so two stores never conflict. `push` is opt-in and runs `audit` first, every attempt: pushed evidence is permanent in every clone, and a leak is fixed only by rewriting history.

## 5. The index and the public views

The index (`crates/git-evidence/src/index/`) is a disposable SQLite cache of the folded notes. Before each answer it compares the refs' tips with its pin and re-folds only notes whose blobs changed (`crates/git-evidence/src/index/catch_up.rs`); then the question reads one snapshot with the pin from that snapshot (`crates/git-evidence/src/index/handle.rs`).

- **Per-build files.** Each build has its own file under `<git-common-dir>/evidence/` (`crates/git-evidence/src/index/per_build.rs`), so binaries from different source sharing one store never refold one file in turn. A file is removed only when no connection can hold it (its `.sqlite.lock` taken without waiting); when it counts as unused is the README's "Storage".
- **Locks and time limits.**

| Lock or limit                            | Scope                                                                                   | Code                                                      |
| ---------------------------------------- | --------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| one lock file per notes ref, blocking    | writers of a kind in one store                                                          | `Store::lock` in `crates/git-evidence/src/store.rs`       |
| one `remote` lock                        | `fetch` and `push` across workspaces of a store                                         | `crates/git-evidence/src/share/remote.rs`                 |
| `.sqlite.lock`, held shared while open   | proves a file unused before removal                                                     | `crates/git-evidence/src/index/per_build.rs`              |
| SQLite busy wait, 30 s                   | other processes of one build waiting on its fold                                        | `crates/git-evidence/src/index/handle.rs`                 |
| read time limit, 30 s (`--time-limit`)   | every answer and `sql`; past it the read is stopped and refused, releasing its snapshot | `TIME_LIMIT` in `crates/git-evidence/src/index/handle.rs` |
| WAL truncated past 64 MiB after a commit | keeps one long reader from growing the log without bound                                | `crates/git-evidence/src/index/handle.rs`                 |

- **A fold is one transaction**, so a reader sees the previous complete fold, never empty tables. `record` folds only what it needs to find runs (`Reach::Runs`) and leaves the landings to the next answer.
- **Counted runs.** A run whose commit is a landing with another tree is misfiled; it is a violation and in no view (`counted_runs`, `crates/git-evidence/src/index/schema.rs`).
- **Public views, schema 6** (`PUBLIC_SCHEMA` in `crates/git-evidence/src/ask/sql.rs`; pinned by `crates/git-evidence/tests/public_schema.snap`): `about`, `landings`, `runs`, `params`, `conditions`, `measures`, `verdicts`, `outcomes`. Their names, columns and types change only with the schema version; the index's own tables change with `VIEWS_VERSION` and are not a contract. `runs.conditions` is `disturbed` when the close's `conditions.disturbed` is set, else `quiet`. Every other condition is the producer's provenance: the `conditions` view holds each value as a row by its dotted path (the leaves `show RUN` prints), and reads no meaning into it. A producer that relates one run to another, keeps raw output elsewhere, or times the steps inside a run says so in its own close conditions; the tool adds no field for it. `sql` runs one statement on a read-only connection and refuses one that writes or returns no rows; `sql --path` prints the file for `sqlite3` or DuckDB.

## 6. The command line and its contract

`crates/git-evidence/src/main.rs` parses, asks the Rust API and prints; it computes nothing, so a Rust caller gets every row the CLI shows. The grammar is `crates/git-evidence/src/cli/args.rs`; the manual pages render from it (`crates/git-evidence/src/cli/manual.rs`), so a page cannot say what `--help` does not.

```text
read    git evidence [--last N] · show REV|RUN · log REV|A..B|SELECT · trend METRIC KEY · diff A B · keys · sql
write   record KIND · fetch · push [--dry-run] · audit · bench
```

- **Rows, nothing else.** A header, then one line per fact, oldest first. Aligned columns at a terminal (`crates/git-evidence/src/cli/columns.rs`), RFC 4180 CSV with `--csv` (`crates/git-evidence/src/cli/csv.rs`); the columns are the same (`TidyRows`, `crates/git-evidence/src/rows.rs`). No summary, colour, verdict or stderr on success. A value not held is an empty cell, never 0.
- **Exit status**, declared once in `crates/git-evidence/src/cli/manual.rs`: 0 rows printed (perhaps only the header); 1 could not run; 2 nothing recorded for what was named (key, metric, run, revision with no run, range end off the trunk; `NotRecorded`, `crates/git-evidence/src/ask/not_recorded.rs`, with the nearest names), and also a refused `record`, `fetch` or `push` and an `audit` with findings.
- **Rust.** `Evidence` is the one entry point (`crates/git-evidence/src/evidence.rs`); reads go through `Evidence::at` with a query value per verb, returning rows or `Error::NotRecorded`. It hands out no database handle and no Git writer. The crate root is the whole API, pinned by `crates/git-evidence/tests/contract.rs`.
- **Revisions.** In a jj workspace a revision is a jj revset first; one jj cannot resolve is a Git revision, with `HEAD` meaning `@-`; other spellings of the shared `HEAD` are refused (`crates/git-evidence/src/cli/revision.rs`). Shallow and partial clones are refused.
- **Self-measurement.** `bench` times this binary on a generated or copied store nobody else reads and can return its figures as a run of kind `tool` (`crates/git-evidence/src/bench/`).

## 7. Left out on purpose

- **Verdicts.** No `regressed`, `owed`, `status`, bounds, judges, expectations, facts check or `evidence/config.toml`; no answer envelope (pin, coverage, caveats, found/partial/empty/unavailable, exit 3); no `--json`; no trend summary. Whether a number got worse is a policy, and policy belongs to the project that holds it, as queries or scripts over the rows. One machine format (CSV) cannot drift from a second. Versions developed before the first release judged (regressions, bounds, expectations); every such judgement embedded one project's policy in a shared tool and had to be explained by caveats, so 0.1.0 ships the rows alone.
- **Choosing silently.** Where `diff` needs one number per side it takes the newest complete run of the series and names the run and the count it chose from.
- **Retention.** A landing's tree is kept alive by the trunk; a candidate's note may outlive its tree.
- **Raw bodies, running, serving.** A run's `source` names an original record; the producer keeps its own raw output. git-evidence is not a runner or a server.

## 8. Two crates

`git-evidence` reaches Git only through its command line, and every call goes through one boundary, `crates/git-process/`: one invocation prefix (no automatic gc, objects and refs flushed before a command returns, a fixed locale, no prompts, and `isolated`, which strips the variables that would redirect Git), typed object ids validated where they are parsed, one UTC timestamp form, and process starts retried when the operating system refuses them for lack of resources, with each failed attempt reported to an optional observer. It knows nothing about evidence, so other programs that drive `git` can use it alone.

`git-evidence` re-exports `git-process`'s object ids, `IdError` and `Timestamp`, so `git-process` is part of its Rust API. The two crates live in one repository and are released together at one version; `git-process` is published first.
