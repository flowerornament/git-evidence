# git-evidence

Numbers recorded per Git commit, kept in Git notes.

A producer (a benchmark, a test suite, a CI step) records what it measured on a commit: a latency, a byte count, a pass or a fail, or why it could not measure. `git-evidence` keeps those records in Git notes beside the code, keyed by the commit's tree, merges them between clones without conflicts, and prints them back as plain rows: aligned columns at a terminal, CSV with `--csv`. It judges nothing. Whether a number got worse is for you to decide, with `sort`, `awk`, a script or an SQL query over the rows.

It needs nothing but Git: no server, no database to run, no files in the working tree.

```text
commits:                   C1 ───── C2 ───── C3 ───── C4 ──►      each with a tree
                           │T1      │T2      │T2      │T4         (C3 changed nothing: C2's tree)
refs/notes/evidence/perf ──┼────────┼────────┼────────┼──         one notes ref per kind, keyed by tree
                           T1: run 01J…  T2: runs 01K…, 01M…   T4: run 01N… (open)
                                   ▼
         <git-common-dir>/evidence/index-….sqlite   (a disposable cache)
                                   ▼
     git evidence · show · log · trend · diff · keys · sql
```

## Install

```sh
cargo install git-evidence             # or, from a checkout of this crate: cargo install --path .
git evidence --help
```

Git 2.42 or later is required. `git evidence --help` opens the manual once its pages are installed (Revisions, jj and the manual, below).

## Quick start

In a new repository, record one run of a benchmark suite on the first commit, a second run on the next, and read them back. The commit ids below come from a run with fixed author and committer dates; yours will differ. This block runs as a test (`tests/readme_quickstart.rs`).

```console
$ git init -q -b main
$ echo 'fn parse() {}' > parser.rs && git add parser.rs && git commit -q -m "parser: first version"
$ git config evidence.machine laptop
$ git evidence record perf <<EOF
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":0,"time":"2026-10-08T03:10:02.123Z","event":"open","commit":"$(git rev-parse HEAD)","producer":{"name":"bench-suite"},"suite":{"harness":"bench-suite/2","params":{"repeats":20},"metrics":{"latency.p99_ns":{"unit":"ns","better":"lower"}}},"environment":{"machine":"laptop"},"keys":["parse_large","parse_small"]}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":1,"time":"2026-10-08T03:10:09.500Z","event":"result","key":"parse_large","outcome":{"measured":[{"metric":"latency.p99_ns","value":88634,"n":20,"mad":310}]}}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":2,"time":"2026-10-08T03:10:11.000Z","event":"result","key":"parse_small","outcome":{"measured":[{"metric":"latency.p99_ns","value":1210,"n":20,"mad":15}]}}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":3,"time":"2026-10-08T03:10:11.200Z","event":"close","end":"complete"}
EOF
run                         written  already_present  run_bytes
01JA8Z3K4M0000000000000000        4                0        937
$ echo 'fn parse() { fast() }' > parser.rs && git commit -q -am "parser: faster"
$ git evidence record perf <<EOF
{"v":2,"run":"01JA8Z3K4M0000000000000001","seq":0,"time":"2026-10-09T03:10:02.123Z","event":"open","commit":"$(git rev-parse HEAD)","producer":{"name":"bench-suite"},"suite":{"harness":"bench-suite/2","params":{"repeats":20},"metrics":{"latency.p99_ns":{"unit":"ns","better":"lower"}}},"environment":{"machine":"laptop"},"keys":["parse_large","parse_small"]}
{"v":2,"run":"01JA8Z3K4M0000000000000001","seq":1,"time":"2026-10-09T03:10:09.500Z","event":"result","key":"parse_large","outcome":{"measured":[{"metric":"latency.p99_ns","value":80412,"n":20,"mad":290}]}}
{"v":2,"run":"01JA8Z3K4M0000000000000001","seq":2,"time":"2026-10-09T03:10:11.000Z","event":"result","key":"parse_small","outcome":{"refused":{"reason":"noisy_host"}}}
{"v":2,"run":"01JA8Z3K4M0000000000000001","seq":3,"time":"2026-10-09T03:10:11.200Z","event":"close","end":"complete"}
EOF
run                         written  already_present  run_bytes
01JA8Z3K4M0000000000000001        4                0        897
$ git evidence
landing  commit        date                  perf  subject
      0  2f263a8ba34b  2026-10-08T03:00:00Z     1  parser: first version
      1  11730712b4f6  2026-10-08T03:00:00Z     1  parser: faster
$ git evidence show HEAD
kind  machine  key          metric          value       unit  run
perf  laptop   parse_large  latency.p99_ns  80412       ns    01JA8Z3K4M0000000000000001
perf  laptop   parse_small  refused         noisy_host        01JA8Z3K4M0000000000000001
$ git evidence log HEAD
landing  run                         kind  producer     machine  opened                    ended     measured  verdict  refused  skipped
      1  01JA8Z3K4M0000000000000001  perf  bench-suite  laptop   2026-10-09T03:10:02.123Z  complete         1        0        1        0
$ git evidence trend latency.p99_ns parse_large
landing  commit        date                  machine  value  unit  run
      0  2f263a8ba34b  2026-10-08T03:00:00Z  laptop   88634  ns    01JA8Z3K4M0000000000000000
      1  11730712b4f6  2026-10-08T03:00:00Z  laptop   80412  ns    01JA8Z3K4M0000000000000001
$ git evidence diff HEAD~1 HEAD
kind  machine  variant   key          metric          before  after  change  unit  before_runs  after_runs  before_run                  after_run
perf  laptop   ecd56f15  parse_large  latency.p99_ns   88634  80412   -9.3%  ns              1           1  01JA8Z3K4M0000000000000000  01JA8Z3K4M0000000000000001
perf  laptop   ecd56f15  parse_small  latency.p99_ns    1210                 ns              1              01JA8Z3K4M0000000000000000
$ git evidence show 01JA8Z3K4M0000000000000001
kind  machine  key          metric          value          unit  run
perf  laptop                param.producer  bench-suite          01JA8Z3K4M0000000000000001
perf  laptop                param.harness   bench-suite/2        01JA8Z3K4M0000000000000001
perf  laptop                param.repeats   20                   01JA8Z3K4M0000000000000001
perf  laptop   parse_large  latency.p99_ns  80412          ns    01JA8Z3K4M0000000000000001
perf  laptop   parse_small  refused         noisy_host           01JA8Z3K4M0000000000000001
$ git evidence sql "SELECT r.landing, m.key, m.value FROM measures m JOIN runs r USING (run) ORDER BY 1, 2"
landing,key,value
0,parse_large,88634
0,parse_small,1210
1,parse_large,80412
```

A run is event lines: `open` names the commit, the suite and its metrics (each with a unit and which direction is better), each `result` records one key's outcome, and `close` ends the run. Recording Runs, below, has the grammar. `git evidence` with no verb lists the newest landings, oldest first: the commits on the trunk's first-parent line, numbered from the oldest.

## Reading

```text
git evidence [--last N]                            the newest landings (default 10): runs of each kind, subject
git evidence show REV|RUN [--kind G] [--machine G] [--key G] [--metric G]
                                                   one row per number or outcome on a revision, or in one run
git evidence log REV | A..B | SELECT               one row per run on REV's tree, or on each selected landing
git evidence trend METRIC KEY [A..B] [SELECT] [--machine M]
                                                   one row per run's value over the landings (default --last 30)
git evidence diff A B [--kind K] [--key K] [--metric G] [--machine M]
                                                   the numbers that differ between two revisions
git evidence keys [--kind K] [--metric G]          one row per key and metric recorded
git evidence sql QUERY | --path                    one read-only SQL query (SQL, below), or the index file
```

`G` is a glob (`*`, `?`, `[…]`). `SELECT` is `--since DATE`, `--until DATE` and `--last N`: the landings committed on or after, on or before (`DATE` is `YYYY-MM-DD`, a whole UTC day, or `YYYY-MM-DDTHH:MM:SSZ`), and the newest N of what the others leave; a range `A..B` is the landings after A, through B, both landings. Every verb takes `--csv`, `--trunk REF` (the ref whose first-parent line is the landings; default `origin/HEAD`'s target, else the first of `origin/main`, `origin/master`, `main`, `master`) and `--time-limit SECONDS` (default 30: a read that runs longer is stopped).

- **`show`** prints, for every run on the revision's tree, one row per recorded number, and one row per key that recorded no number: its `metric` is the outcome (`verdict`, `refused`, `skipped`) and its `value` the verdict's status (`pass`, `fail`, `timeout`) or the reason. Given a run id instead, it prints that run, its parameters first (`metric` is `param.NAME`) and its close conditions, one row per value (`metric` is `condition.PATH`, as `condition.load`).
- **`log`** prints one row per run: how it ended (`complete`, `refused: REASON`, empty while open) and how many keys each outcome has.
- **`trend`** prints one row per ended run that recorded the metric for the key, oldest landing first. Several runs on one landing are several rows, in the order they opened.
- **`diff`** matches the numbers of two revisions on kind, machine, variant, key, metric and unit, and prints those that differ. `variant` is the first 8 characters of the series' digest, so two series of one key never look alike (`show RUN` prints a run's parameters). Where a revision has several complete runs of one series, the newest gives the number: `before_runs` and `after_runs` count them and `before_run` and `after_run` name the one taken, so a choice is never silent. A number on one side only, or in another unit, keeps the other side empty; units are never converted. `change` is a percentage of `before`, or the plain difference where `before` is 0 or the unit is already a ratio (`1`) or a percentage (`%`).

## Output

Every verb prints rows: a header line, then one line per fact. At a terminal the columns are aligned, numbers to the right, each number shown with at most 6 significant digits (`474.183`; an integer whole, `101529`; beyond 10^-4 to 10^15 in exponent form, `1.2e-7`) and a commit shortened to 12 characters; with `--csv` the same columns are CSV (RFC 4180), each number and commit whole. Rows run oldest first: landings and runs in the order they happened. Nothing else is printed: no summary, no colour, no verdict, and nothing on standard error when the command succeeds. A value the store does not hold is an empty cell, never a zero. Only ended runs give values: an open run has none yet.

| Verb                     | Header                                                                            |
| ------------------------ | --------------------------------------------------------------------------------- |
| `git evidence` (no verb) | `landing,commit,date,KIND…,KIND_refused…,subject`                                 |
| `show`                   | `kind,machine,key,metric,value,unit,run`                                          |
| `log`                    | `landing,run,kind,producer,machine,opened,ended,measured,verdict,refused,skipped` |
| `trend`                  | `landing,commit,date,machine,value,unit,run`                                      |
| `diff`                   | `kind,machine,variant,key,metric,before,after,change,unit,before_runs,after_runs,before_run,after_run` |
| `keys`                   | `kind,key,runs,metric,unit,direction`                                             |
| `record`                 | `run,written,already_present,run_bytes`                                           |
| `audit`                  | `section,name,kind,tree,run,seq,notes,lines,bytes,detail`                         |
| `fetch`, `push`          | `remote,reference,change,detail`                                                  |
| `bench`                  | `verb,metric,value,unit,n,mad,outcome,detail`                                     |
| `sql`                    | the query's own columns                                                           |

The overview has one column per kind recorded on the landings it lists (its run count), then a `KIND_refused` column for each kind with a refused run there. `keys` lists a key that recorded no number (a verdict without durations) with its `metric`, `unit` and `direction` empty: the key is recorded, it has no metric. A metric its runs declared two ways (two units, or a direction and none) is listed once per declaration.

**Exit status.** 0: the rows were printed (the header alone when no row matched). 1: the command could not run: a malformed command, a revision that does not resolve, no trunk, a store it cannot read; standard error says why. 2: nothing is recorded for what the command named (a key, a metric, a run, a revision with no run, a range end that is not a landing); standard output is empty and standard error names it with the nearest recorded names. A refused `record`, `fetch` or `push`, and an `audit` with findings, also exit 2.

## The model

| Noun        | Is                                                                                                                                                                   |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| commit      | what a producer ran on. A run names it; its tree is what the run is about.                                                                                           |
| tree        | the code, as a Git tree id. Evidence is about trees: a candidate that lands unchanged has the landing's tree, so it carries its evidence with it.                     |
| trunk       | the ref whose first-parent line is the landings (default: `origin/HEAD`'s target).                                                                                   |
| landing     | a commit on the trunk's first-parent line. Landings are numbered from the oldest, `0` first.                                                                         |
| kind        | what a piece of evidence is: `perf`, `tests`, `checks`. The namespace readers select by; one notes ref each.                                                         |
| run         | one execution of a producer, about one tree, identified by a [ULID](https://github.com/ulid/spec).                                                                   |
| machine     | where a run executed, by the name the store's Git config gives it (`evidence.machine`).                                                                              |
| key         | one thing a run answers: a benchmark, a test, a check step, named as its producer names it (1–1024 bytes, no control characters, no surrounding whitespace).                                    |
| metric      | a named quantity a run declares once, with its unit and, optionally, which direction is better.                                                                     |
| outcome     | a key's result: `measured`, `verdict`, `refused` or `skipped` (below).                                                                                               |
| variant     | what must match for two values to be matched by `diff`: kind, producer, harness, suite params, build profile, machine, environment params. Derived; producers never hash. |

Nothing else has an identity: there are no writer names, store ids or method hashes. The tree is the subject, the run id is the evidence, and the kind is the namespace.

### Outcomes

| Outcome           | Means                                                                                                    |
| ----------------- | -------------------------------------------------------------------------------------------------------- |
| `measured`        | values, each with a declared metric, optional `n` (samples) and `mad` (median absolute deviation)        |
| `verdict`         | `pass`, `fail` or `timeout`, with the individual `tries`, each with its wall and CPU times if recorded   |
| `refused{reason}` | no value; retrying may succeed (`noisy_host`, `timeout`, `disk_low`, `interrupted`)                      |
| `skipped{reason}` | no value, and none is possible at this code (`build_failed`, `compile_error`)                            |

A try carries `status` and, when recorded, `duration_ms` (wall time) and `cpu_ms` (CPU time: user plus system time of the process that ran the try and the descendants it waited for, in milliseconds). An absent time was not recorded; it is never zero. Both are finite and not negative, and `cpu_ms` may exceed `duration_ms` when the work ran in parallel.

The index derives each time as a metric per status, the sum over the verdict's tries: `verdict.<status>.duration_ms` and `verdict.<status>.cpu_ms` for `pass`, `fail` and `timeout` (unit `ms`, lower is better). A time is derived only when every try of the verdict has it and the sum is finite. A run cannot declare a metric under `verdict.`.

## SQL

`git evidence sql "<query>"` brings the index up to date with the notes and the trunk, then runs one statement that reads rows on a read-only connection, in one snapshot. It prints the rows as CSV (a header, then one line per row; NULL is an empty field), and nothing on standard error. A statement that writes, or that returns no rows, is refused, and so are two columns of one name. `git evidence sql --path` refreshes the index and prints its file's path, for `sqlite3` or DuckDB. A reader that opens that file in another tool holds none of `git-evidence`'s locks, so another build may remove the file from under that reader once the file has gone unused for an hour or a day (Storage, below); copy the file to keep it for longer.

**Rows are the recorded numbers.** They say how each run ended (`ended`), under what conditions (`conditions`) and in which variant (`variant`); comparing them is the query's.

The public views are schema 6 (`about.schema_version`, and the file's `PRAGMA user_version`). Their names, columns and types change only with a new schema version. Every other table is the index's own and changes with its views version.

A column is named as the other verbs' columns name the same value: `run`, `tree`, `landing`, `key`. The one exception is `commit_id`, since `commit` is a reserved word in SQL.

| View       | One row per                    | Columns                                                                                                                                                                                                              |
| ---------- | ------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `about`    | the index (one row)            | `schema_version`, `views_version`, `trunk_ref`, `trunk` (the trunk's commit the rows were read at; NULL when the ref does not resolve), `public_views`, `pending_views` |
| `landings` | landing                        | `landing` (its position: `0` is the oldest), `commit_id`, `tree`, `date` (committer time, `YYYY-MM-DDTHH:MM:SSZ`), `subject`                                                                                                                                |
| `runs`     | run                            | `run`, `kind`, `producer`, `machine` (NULL when the run names none), `variant`, `conditions` (`disturbed` or `quiet`), `disturbed` (its reason), `landing` (the first landing with the run's tree; NULL off the trunk), `tree`, `commit_id`, `opened`, `closed` (close time; NULL while open), `ended` (how the run said it ended: `complete` or `refused`; NULL while open), `reason` |
| `params`   | run parameter                  | `run`, `name`, `value` (suite and environment parameters; JSON text when not a string)                                                                                                                               |
| `conditions` | value a run closed with      | `run`, `path` (dotted: `cause.class`, an array element by its index, `findings.0.id`), `value` (text; JSON text when not a string; NULL for `null`); exactly the rows `show RUN` prints as `condition.PATH`, so a path a field name with a `.` also spells appears once per value                  |
| `measures` | value                          | `run`, `key`, `metric`, `value`, `unit`, `direction` (`lower`, `higher` or NULL), `n`, `mad`, `origin` (`recorded`, `derived`) |
| `verdicts` | verdict                        | `run`, `key`, `status`, `duration_ms`, `cpu_ms` (sums over the tries; NULL unless every try has one), `tries` |
| `outcomes` | key a run answered             | `run`, `key`, `outcome` (`measured`, `verdict`, `refused`, `skipped`), `status`, `reason`, `detail`, `location` |

Booleans are 0 or 1. Filter landings by `landing` and `date`. These examples run as tests against a fixture store (`tests/readme_sql.rs`):

```sql
-- ours against the reference per key, in the newest complete run on laptop that measured ours
WITH newest AS (
  SELECT r.run FROM runs r
  WHERE r.kind = 'perf' AND r.machine = 'laptop' AND r.ended = 'complete'
    AND EXISTS (SELECT 1 FROM measures m WHERE m.run = r.run AND m.metric = 'latency.p99_ns')
  ORDER BY r.landing DESC, r.closed DESC LIMIT 1)
SELECT m.key, m.value AS ours, f.value AS reference, round(m.value / f.value, 2) AS ratio,
       r.landing, r.conditions
FROM newest JOIN runs r USING (run)
JOIN measures m ON m.run = r.run AND m.metric = 'latency.p99_ns'
JOIN measures f ON f.run = r.run AND f.key = m.key
  AND f.metric = 'reference.latency.p99_ns'
ORDER BY ratio DESC;
```

```sql
-- verdict time per key of the full suite over the last 30 landings
SELECT l.landing, v.key, v.status, v.duration_ms, v.cpu_ms
FROM verdicts v JOIN runs r USING (run) JOIN landings l USING (landing)
JOIN params p ON p.run = r.run AND p.name = 'suite' AND p.value = 'full'
WHERE r.kind = 'checks' AND r.ended = 'complete'
  AND l.landing > (SELECT max(landing) - 30 FROM landings)
ORDER BY l.landing, v.key;
```

```sql
-- keys refused or skipped on the landings of October 2026
SELECT l.landing, r.kind, o.key, o.outcome, o.reason
FROM landings l JOIN runs r USING (landing) JOIN outcomes o USING (run)
WHERE l.date >= '2026-10-01' AND l.date < '2026-11-01'
  AND o.outcome IN ('refused', 'skipped')
ORDER BY l.landing, o.key;
```

## Writing

```text
git evidence record KIND < events.jsonl            write event lines of one kind
git evidence fetch [REMOTE]                        others' evidence, union-merged into this store's
git evidence audit                                 what a push would publish: sizes, unreadable runs, personal text
git evidence push [REMOTE] [--dry-run]             this store's evidence, after a clean audit
git evidence bench [--this-store] [--landings N] [--runs N] [--repeats N] [--returned PATH --on REV]
                                                   time this binary on a store nobody else reads (Benchmark, below)
```

`REMOTE` is a configured remote name (default: Git config `evidence.remote`, else `origin`); a URL or a path is refused.

## Recording runs

A run is JSON lines, one event each, with the grammar `open · declare? · result* · close?`:

```jsonc
{"v":2,"run":"01JA8Z3K4M…","seq":0,"time":"2026-10-08T03:10:02.123Z","event":"open",
 "commit":"9c41e2…",                                       // the commit run on; its tree is the note key
 "producer":{"name":"bench-suite","version":"1.4.0"},
 "suite":{"harness":"bench-suite/2","params":{"repeats":20},
          "metrics":{"latency.p99_ns":{"unit":"ns","better":"lower"}}},
 "environment":{"machine":"ci-runner","params":{"host_role":"dedicated"},"descriptor":{"cpu":"…","os":"…"}},
 "build":{"profile":{"target":"x86_64-unknown-linux-gnu","flags":"release"},"toolchain":"1.94.0","artifact":"sha256:5d0b…"},
 "source":"results/2026-10-08/run-17.json"}                // optional: the original record this run imports
{"v":2,"run":"01JA8Z3K4M…","seq":1,"time":"…","event":"declare",
 "keys":["parser/parse_large","lexer/tokens"],"metrics":{"rss_bytes":{"unit":"By","better":"lower"}}}
{"v":2,"run":"01JA8Z3K4M…","seq":2,"time":"…","event":"result","key":"parser/parse_large",
 "outcome":{"measured":[{"metric":"latency.p99_ns","value":88634,"n":20,"mad":310},{"metric":"rss_bytes","value":4.2e7}]},
 "location":"benches/parser.rs:118"}
{"v":2,"run":"01JA8Z3K4M…","seq":3,"time":"…","event":"result","key":"lexer/tokens",
 "outcome":{"verdict":{"status":"pass","tries":[{"status":"fail","duration_ms":812},{"status":"pass","duration_ms":790,"cpu_ms":2310}]}}}
{"v":2,"run":"01JA8Z3K4M…","seq":4,"time":"…","event":"close","end":"complete",
 "conditions":{"load":0.4}}
```

| Event     | Carries                                                                                                                                                                                                        |
| --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `open`    | `commit`; `producer` (`name`, `version`?); `suite` (`harness`, `params`, `metrics`); `environment` (`machine`, `params`, `descriptor`); `build` (`profile`, `toolchain`, `artifact`); `keys`?; `source`?           |
| `declare` | at most once, before any result: `keys` (when `open` did not name them), `metrics` learned late, `build`                                                                                                       |
| `result`  | `key`, `outcome` (one of `measured`, `verdict`, `refused`, `skipped`), `location`? (the code site that produced it)                                                                                             |
| `close`   | `end`: `"complete"` (every declared key answered) or `{"refused":{"reason":…}}` (every unanswered key refused with that reason); `conditions`? (readings taken during the run)                                |

Every line carries `v` (2), `run`, `seq` (0, 1, 2, … without gaps in each run) and `time` (`YYYY-MM-DDTHH:MM:SS.mmmZ`, the one form that sorts as text).

The rules `record` enforces:

- **Metrics are declared once per run,** in `suite.metrics` or `declare.metrics`, with a [UCUM](https://ucum.org/) unit and an optional `better` (`lower` or `higher`). A measure carries only `metric`, `value`, `n` and `mad`. A measure of an undeclared metric, a redefinition (another unit or direction) and a declared `verdict.` name are refused.
- **A result answers a declared key once.** `close` is the last event; nothing can be added behind it, because a run is read only up to its first missing `seq`.
- **Numbers are finite.** A value, spread, duration or CPU time that is not finite is refused rather than written as `null`. A negative wall or CPU time in a verdict try is refused.
- **A run is about one tree.** A run id seen on two trees is refused where `record` can see it, and left out of every row where it cannot.
- **Budgets.** Git config `evidence.<kind>.budget` caps the bytes one run of that kind may take.

`record` resolves each run's tree from its `open` (or from where the run is already recorded), checks the whole batch, appends, and returns only once the ref and its directory are on disk. A batch is all-or-nothing, across trees too: the ref moves once (a batch over several trees is staged first). Lines already present are ignored, so delivering a batch twice is harmless. It prints one row per run in the batch: the `run`, its lines `written` and `already_present`, and its size once the batch is in (`run_bytes`). A batch delivered again prints the same runs, every line already present. An input holding no event line is refused (exit 2): nothing was recorded.

**Open first.** A producer opens a run before it works, so a crash leaves a visible unfinished run, never a silent gap. A producer that restarts after a crash closes its own unfinished runs with `"end":{"refused":{"reason":"interrupted"}}`. A run nobody closes stays open, and gives no values.

**Close conditions** are free JSON, kept as provenance. One is read:

- `"disturbed": "<reason>"`: the values were taken under disturbed conditions (load, noise, a drifting machine). They stand; the `runs` view's `conditions` column says `disturbed`.

Every other condition (`load`, or a project's own such as `verified_base`) is kept as written: `show RUN` prints each value as `condition.PATH`, and the `conditions` view holds the same values, one row each, for a query to join. Neither reads a meaning into them.

**Sources.** An `open` may name its `source`: the original record the run imports (an id, an immutable path or a digest), never where a copy was found. It is kept as provenance; each run is its own row.

## Benchmark

`git evidence bench` is the tool's own benchmark. It times this binary answering on a store in a temporary directory, so the store in use is never written, and its index is never rebuilt or locked:

- by default, a store generated at a declared size (`--landings`, default 5,000; `--runs`, default 10,000, half perf runs of 16 measured keys and half checks runs of 8 verdicts, spread evenly), the same store on every machine;
- with `--this-store`, a copy of this store: its evidence refs and its trunk, reading its objects through Git alternates.

Its output, one row per figure:

```text
$ git evidence bench
verb      metric   value  unit  n  mad  outcome   detail
overview  cold_ms   <ms>  ms    1    0  measured
overview  warm_ms   <ms>  ms    5 <ms>  measured
trend     warm_ms   <ms>  ms    5 <ms>  measured
sql       warm_ms   <ms>  ms    5 <ms>  measured
record    warm_ms   <ms>  ms    5 <ms>  measured
```

Each figure is the wall time of one process of this binary, process start included: what a person typing the command waits for. **Cold** is the overview with no index, the rebuild every verb pays first. **Warm** is each verb run once untimed, then `--repeats` times (default 5), as the median and its median absolute deviation: the overview, `trend` of the first metric and key the newest landing's runs measured, `sql` of a reference query, and `record` of one perf run of 16 keys on the newest landing. A verb that cannot answer (exit 1, or a `record` refused) is reported failed, naming why; every other exit of a question is an answer.

`--returned PATH --on REV` also writes the figures to PATH as one run of kind `tool` about REV (the code whose `git-evidence` was measured), on this store's machine, opened before the work as a producer's run is: `git evidence record tool < PATH` records it. Its keys are the verbs (metrics `cold_ms`, `warm_ms`) and `store` (metrics `landings`, `runs`); a verb that failed is `refused` `answer_failed`, and a bench that cannot measure closes the run `refused` `bench_failed`. Its parameters are `store` (`generated` or `existing`), `repeats` and, generated, `landings` and `runs`, and the instrument: `clock = wall`, `process_start = included`, `compile = excluded` and `profile` (`debug` or `release`); so only figures of one store, size and instrument share a variant. Read them back with `trend` or `sql` like any other numbers.

The generated store's notes are written in bulk with `git fast-import`, not through `record`; a test (`src/bench/generate.rs`) checks that they are the bytes `record` writes for the same runs.

## Storage

```text
refs/notes/evidence/<kind>                                 one note per tree: every run's lines; written here, pushed
refs/notes/remotes/<remote>/evidence/<kind>                fetched copies; never branches or bookmarks
<git-common-dir>/evidence/index-v<views>-<source>.sqlite   the disposable index of one build, with its .sqlite.lock
<git-common-dir>/evidence/locks/                           one lock per notes ref, and one for fetch and push
```

```text
$ git notes --ref evidence/perf show 'HEAD^{tree}'
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":0,"time":"2026-10-08T03:10:02.123Z","event":"open",…}
{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":1,"time":"2026-10-08T03:10:09.500Z","event":"result",…}
```

- **Notes are keyed by tree id** in Git's own fanout layout, so an append costs the same at 10 or 100,000 notes.
- **One ref per kind, shared by every writer.** Events are immutable and unique, so a note is a set of lines. Stores that appended concurrently are reconciled with `git notes merge -s cat_sort_uniq`, which is exactly set union: nothing conflicts and nothing is forced. Within one store, writers serialise on a lock file.
- **Line order inside a note is irrelevant.** The fold orders each run by `seq`.
- **The index is a cache.** On each question it compares the refs' tips with those it last read and re-folds only the notes that changed. It is safe to delete. Each build of `git-evidence` (views version and source) has its own file, so binaries built from different source sharing a store never rebuild each other's file or wait on its lock; a new file removes another build's file only when no connection can have it open (its `.sqlite.lock` taken without waiting, or, for a file an older binary made, no `-wal` or `-shm`), and only once it has gone unused for a day, or for an hour while three more recently used ones are kept. A binary's first question folds its file from the notes, in one write transaction, so a reader of the file sees no partial fold: about 1.7 s on the generated 5,000-landing store and 4 to 6 s on a 4,636-landing store in use (Mac laptop, wall time, load 15 to 20, release, 2026-11-02). Other processes of that build asking meanwhile wait on the file's lock, for at most 30 s. `record` folds only the notes it needs to find runs and leaves the landings to the next answer.
- **No retention.** A landing's tree is kept alive by the trunk. A candidate that never landed may have its tree collected; its note survives, unreachable from any landing.

### Sharing

`fetch` copies each kind's ref into `refs/notes/remotes/<remote>/evidence/<kind>` and union-merges it into the local ref. `push` asks the remote what each kind's ref holds and sends only what it lacks: a ref the remote holds at this store's commit costs nothing. If the remote holds evidence this store has not merged (another store pushed first), it fetches, merges and tries again, up to three times. Git never prompts; a remote that asks for credentials refuses.

Pushed evidence is permanent in every clone, so `push` is refused until `git config --local evidence.push true`, and every push audits what it sends first: every note version the remote does not hold yet, removed notes included, judged beside the version the remote holds of the same tree. Each ref is sent by the commit audited, and only while the remote still holds what it held when asked, so nothing unaudited leaves. `audit` judges the whole history, as a first push sends it; it reports each ref's notes, lines and bytes, and as findings any run that does not fold and any text that looks personal: this machine's host names, the user's login and home, paths and addresses. Names the store publishes itself (its kinds, its configured and recorded machine names) are public vocabulary and not findings; login names and homes never are. While there is a finding, nothing is sent. A leak is fixed by rewriting the notes' history, not by removing the note. Run `audit` and `push --dry-run` before the first push.

## Configuration

Store settings are Git config, never files in the tree; all but `evidence.remote` are read from the store's own config (`git config --local`) only:

| Setting                  | Means                                                                         | Default            |
| ------------------------ | ----------------------------------------------------------------------------- | ------------------ |
| `evidence.machine`       | the machine this store's producers run on; required to record from Rust       | none               |
| `evidence.remote`        | the remote `fetch` and `push` use                                             | `origin`           |
| `evidence.<kind>.budget` | the most bytes one run of that kind may take                                  | no budget          |
| `evidence.push`          | `true` allows `push`                                                          | refused until true |

A producer that records its real host name as its machine declares that host name public.

## Revisions, jj and the manual

Installing the binary is enough for Git: Git runs any `git-<name>` on `PATH` as `git <name>`. For jj, set an alias once per repository:

```sh
jj config set --repo aliases.evidence '["util", "exec", "--", "git-evidence"]'
```

A `REV` is a Git revision. Inside a jj workspace it is a jj revset first; a revision jj cannot resolve (`HEAD`, `HEAD~2`, `origin/main`) is read as a Git revision of the store, with `HEAD` meaning the workspace's `@-` (the shared store's own `HEAD` names another workspace's checkout, so it is never read). A `RUN` is a run id. The tool finds the shared Git store from any working directory; a jj workspace without `.git` follows `.jj/repo` to its store's `git_target`. `GIT_EVIDENCE_GIT_DIR` overrides discovery. Shallow and partial clones are refused: evidence needs the history and the note blobs locally. Git 2.42 or later is required.

**The manual.** `git evidence --help` is Git asking `man git-evidence`. The binary renders its pages from its own command line, so they say what `-h` says: `git-evidence manual DIR` writes `git-evidence.1` and one `git-evidence-<verb>.1` per verb; put them in a `man1` directory on the manual path (`manpath` lists it), such as `~/.local/share/man/man1` beside `~/.local/bin`. `git evidence <verb> --help` is the binary's own help, and `--help` (unlike `-h`) ends with the exit status.

## From Rust

`Evidence` is the one entry point. A producer opens a run before it works; each metric is declared once, with its unit and direction:

```rust
use git_evidence::{Direction, End, Measure, Metric, MetricName, Open, Outcome};
# let scratch = git_evidence::testing::Scratch::new()?.landings(1)?.machine("laptop")?;
# let evidence = scratch.evidence();
# let commit = scratch.landing(0);

let mut open = Open::new(commit, "bench-suite".parse()?, "bench-suite/2", &evidence.machine()?);
open.keys = Some(vec!["parser/parse_large".parse()?]);
open.suite.metrics.insert(
    MetricName::parse("latency.p99_ns")?,
    Metric { unit: "ns".into(), better: Some(Direction::Lower) },
);
let mut run = evidence.run(&"perf".parse()?, open)?;  // durable before any work
run.result(
    &"parser/parse_large".parse()?,
    Outcome::Measured(vec![Measure::new(MetricName::parse("latency.p99_ns")?, 88_634.0)?]),
    None,
)?;
run.close(End::Complete, None)?;                     // refused if a key was left unanswered
# Ok::<(), Box<dyn std::error::Error>>(())
```

A reader asks and gets rows back: an empty list when nothing matched, and `Error::NotRecorded` when the store has nothing recorded for a name it was given:

```rust
use git_evidence::{Error, TrendQuery};
# let scratch = git_evidence::testing::Scratch::new()?.landings(1)?;
# let evidence = scratch.evidence();

let query = TrendQuery::new("latency.p99_ns".parse()?, "parser/parse_large".parse()?); // newest 30 landings
match evidence.at(|at| at.trend(&query)) {
    Ok(points) => {
        for p in &points {
            println!("#{} {} {} {}", p.landing, p.commit, p.value, p.unit);
        }
    }
    Err(Error::NotRecorded(missing)) => println!("{missing}"),
    Err(error) => return Err(error.into()),
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

- `run.finish()` writes the rest of a run (a late declaration, the results, the close) as one all-or-nothing commit.
- `evidence.resume(kind, &id)` finds a run after a crash.
- `evidence.record(kind, &lines)` takes lines built elsewhere; it is `git evidence record`.
- `evidence.at(|at| …)` asks questions in one read of the store, one method per verb, each taking its query (`TrendQuery`, `LogQuery`, `ShowQuery`, `KeysQuery`, `DiffQuery`). `at.overview(n)` lists the newest landings, including those without runs, each with its run counts; `at.log` lists the runs on a tree or on selected landings, `at.show` the runs a revision or a run id names, and `at.run(&id)` one run.
- A refused write is `RecordError::Refused(Refusal)`, naming the rule and its remedy.
- Every answer and every report implements `TidyRows`: `value.header()` and `value.rows()` are what the command line prints.
- `evidence.sql(query)` returns `Rows`, the rows of one read-only query (SQL, above).
- `git_evidence::testing::Scratch` (feature `testing`) gives an integration's tests a throwaway repository, as these examples use.
- The `bench` feature, on by default and needed by the binary, carries the `bench` verb's types; a library user can leave it out with `default-features = false`.

## Guarantees

Each is enforced by tests in the named file of this crate.

| Guarantee                                                                                                                                 | Test                                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| **Grow-only.** A ref's events only accumulate; merging two stores gives their union, so `fetch` and `push` never lose evidence.           | `tests/remote.rs`                                         |
| **Idempotent.** A line or a run delivered twice changes nothing: a crashed `record` or `push` can be run again.                           | `src/write/batch.rs`, `tests/remote.rs`, `tests/crash.rs` |
| **Order-free.** Any order of a note's lines folds to the same runs.                                                                       | `src/format/fold.rs`                                      |
| **Ended runs answer every declared key.** A `complete` close with keys unanswered is refused at write and unreadable at read.             | `src/write/batch.rs`, `src/format/fold.rs`                |
| **One run, one tree.** A run id seen on two trees is refused where `record` can see it, and counted on neither tree where it cannot. | `tests/reproducible.rs`                                   |
| **Reproducible.** The same notes and trunk give the same rows; nothing reads the clock. Catching up incrementally equals a fresh rebuild. | `tests/reproducible.rs`                                   |
| **Matched within a variant.** `diff` never puts one variant's number beside another's: machines, builds and harnesses are never mixed.    | `tests/diff.rs`                                           |
| **Durable before return.** `record` returns after the objects, the ref and its directory are on disk; a crash leaves all or nothing.      | `tests/crash.rs` (real process aborts)                    |

The crash tests compile fault points into the write path with the `fault-injection` feature, which only this crate's tests enable.

## Stability

The version is 0.x. The event lines (`v: 2`) and the notes layout are permanent: data written under them lives in every clone, and old versions stay readable. The public SQL views change only with their schema version. The CLI and the Rust API may change between 0.x minors, each change recorded in [CHANGELOG.md](CHANGELOG.md).

## Dependencies

Git, through its command line only; every call goes through one hardened boundary. `rusqlite` with bundled SQLite, `serde`, `serde_json`, `sha2`, `thiserror`, `tempfile` (the bench's store) and `clap`, with `clap_mangen` and `roff` to render the manual.

## License

MIT or Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).
