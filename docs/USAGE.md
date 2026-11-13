# Using git-evidence in a project

This is the problem git-evidence was built for, and how a project puts it to work: what records runs, what asks questions, and why each part of the tool is shaped as it is. The manual ([`crates/git-evidence/README.md`](../crates/git-evidence/README.md)) has every verb and column; [`FORMAT.md`](FORMAT.md) has the contract a producer writes to.

## The problem

Picture a project where many contributors, some of them coding agents, land changes on one trunk (the main branch) many times a day; each commit on the trunk's first-parent line is a *landing*. Performance matters, and so does the time the checks take. The numbers exist: a benchmark harness prints them, CI logs hold step times, someone keeps a spreadsheet. But:

- **Nothing joins a number to the code it measured.** A CI log names a job, not a tree; a spreadsheet row names whatever its author typed. Asking "what did this benchmark do over the last 30 landings?" means archaeology.
- **Regressions are noticed late, or never.** Measuring is a manual step, so most landings are never measured, and a slowdown is found weeks later, somewhere in a range of hundreds of commits.
- **"Not measured" looks like "fine".** A landing with no numbers and a landing whose numbers are good are both silent. Nobody can tell a gap from a pass, or why the gap is there (the build failed, the machine was busy, the run timed out).
- **Measurements happen elsewhere and later.** The trustworthy numbers come from a dedicated, quiet machine, not from the laptop that wrote the change, and they arrive after the change has landed, or before it lands, on a candidate whose commit id the landing will not keep.
- **Several machines, clones and tools write.** A benchmark runner, the CI gate and a contributor measuring a candidate all produce numbers, concurrently, from different clones, with different instruments that must never be confused.

git-evidence answers with one store that needs nothing but Git: every run of every producer is recorded on the tree it measured, kept in the repository's notes, merged between clones without conflicts, and read back as rows that a query or a script judges.

## A project's setup, end to end

The examples below file runs under two *kinds* (the namespace a run is recorded under, one notes ref each): `perf` for benchmarks and `checks` for the gate. They run on a dedicated machine named `bench` and a contributor's `laptop`; any names work.

### 1. Every landing gets measured, or says why not

A scheduled job (cron, launchd, a CI schedule) measures one landing per tick on the dedicated machine. It keeps no state of its own: which landing to measure next is a question to the store. On startup it first closes any run it left open in a crash with `"end":{"refused":{"reason":"interrupted"}}`, so the crash counts as one of the landing's tries, and then asks:

```sql
-- the newest of the last 30 landings with no complete or open perf run on its tree,
-- giving up on a landing after 3 refused runs
WITH recent AS (SELECT landing, commit_id, tree FROM landings ORDER BY landing DESC LIMIT 30)
SELECT l.commit_id FROM recent l
WHERE NOT EXISTS (SELECT 1 FROM runs r WHERE r.tree = l.tree AND r.kind = 'perf'
                  AND (r.ended = 'complete' OR r.ended IS NULL))
  AND (SELECT count(*) FROM runs r WHERE r.tree = l.tree AND r.kind = 'perf'
       AND r.ended = 'refused') < 3
ORDER BY l.landing DESC LIMIT 1;
```

The runner opens a run before it starts (so a crash leaves a visible open run, not a silent gap), measures, and records every outcome as a run:

- measured: the values, each metric declared once with its unit and direction;
- not measurable at this code (the build failed): the keys `skipped` with a reason;
- not measured this time (timed out, machine busy, interrupted): the run closed `refused` with a reason, one of the landing's tries.

Then it pushes (`git evidence push`, audited). Now `git evidence --last 30` lists the newest 30 landings with their run counts and refused counts, and `git evidence log A..B` says, landing by landing, what was measured and why something was not. A gap is a fact in the store, never an absence.

The runner and the measuring machine can also be two machines. The measuring machine then needs a clone of the code but none of the evidence: the producer writes its event lines to a file (any program can; `Evidence::run_to_file` does it in Rust), and the runner records that file with `git evidence record perf < run.ndjson`. The run's open becomes visible only once the file is recorded, so the runner, not the measuring machine, is what must survive a crash.

### 2. A candidate is measured before it lands

A contributor wants to know whether a change is faster before landing it. They run the same instrument on the candidate commit and record it. Evidence is keyed by the tree, so:

```console
$ git evidence diff main candidate --kind perf
```

compares the candidate's run with the trunk's, matched on kind, machine, variant, key, metric and unit. When the candidate lands with the same tree (squashed or given a new commit id, with nothing else landed in between), the landing already carries the candidate's run, and the scheduled runner sees nothing left to measure there. If the candidate had to be rebased over other landings, its tree changes, and the landing is measured afresh; the candidate's run stays on its own tree for comparison.

Each run has a `variant`, derived from its kind, producer, harness, suite parameters, build profile, machine and environment parameters. A candidate measured with a changed harness or different suite parameters gets a different variant, so `diff` keeps the two series apart instead of comparing apples with oranges.

### 3. The gate records what it checked and how long it took

The CI or landing gate opens a `checks` run before it executes, records one verdict per step, and then closes the run. A verdict is `pass`, `fail` or `timeout`, with each try (a flaky step's failed try and its passing retry are both kept) and each try's wall and CPU time. Anything else the gate knows goes in the close conditions as free JSON (when each step started, why the landing was refused) and comes back, value by value, in the `conditions` view.

```sql
-- the slowest gate steps over the last 30 landings, with their CPU and retries
SELECT l.landing, v.key, v.status, v.duration_ms, v.cpu_ms, v.tries
FROM verdicts v JOIN runs r USING (run) JOIN landings l USING (landing)
WHERE r.kind = 'checks' AND r.ended = 'complete'
  AND l.landing > (SELECT max(landing) - 30 FROM landings)
ORDER BY v.duration_ms DESC LIMIT 20;
```

### 4. Policy is a saved query, owned by the project

git-evidence never says "regressed". What counts as a regression, which reference a number must beat, which runs to trust: that is the project's policy, and it lives in the project, as SQL files or scripts over the public views, tested against a scratch store (`testing::Scratch`) like any other code. For example, a project that must stay within twice a reference implementation records both in one run and asks:

```sql
-- ours against the reference, per key, in the newest complete run on bench that measured ours
WITH newest AS (
  SELECT r.run FROM runs r
  WHERE r.kind = 'perf' AND r.machine = 'bench' AND r.ended = 'complete'
    AND EXISTS (SELECT 1 FROM measures m WHERE m.run = r.run AND m.metric = 'latency.p99_ns')
  ORDER BY r.landing DESC, r.closed DESC LIMIT 1)
SELECT m.key, m.value AS ours, f.value AS reference, round(m.value / f.value, 2) AS ratio
FROM newest JOIN measures m USING (run)
JOIN measures f ON f.run = m.run AND f.key = m.key AND f.metric = 'reference.latency.p99_ns'
WHERE m.metric = 'latency.p99_ns'
ORDER BY ratio DESC;
```

A nightly script that runs such a query and fails above a threshold is a regression alarm; a release checklist can run another. Changing the policy is a change to a query, reviewed like code, and never a re-recording of evidence.

### 5. Everyone reads the same evidence

Every clone runs `git evidence fetch` to union-merge what the runner and the gate pushed. A contributor on a laptop, an agent in a jj workspace and a dashboard script reading `git evidence sql --path` all see the same rows. Nothing runs as a service.

## Why not a file in the repository?

The obvious design is a `benchmarks.jsonl` in the tree, one line appended per measurement, or a `numbers.json` rewritten on every commit. It works for one person measuring by hand. On a busy trunk it breaks on exactly the forces above:

- **Recording changes what was measured.** A file in the tree is part of the tree. Writing the numbers for commit C makes a new commit C′ with a different tree, which nobody measured; measuring C′ would need C″. Numbers can never sit on the code they describe, only next to it, one commit later.
- **Measurements arrive late and from elsewhere.** A runner that measures each landing on a dedicated machine would have to push a commit to the trunk for every measurement: it competes with contributors for the trunk, needs write access past branch protection and review, and doubles the landings (half of them carrying only data).
- **Concurrent writers conflict.** Two writers appending to one file conflict at its end on every merge or rebase, unless the repository's `.gitattributes` marks it `merge=union` (which keeps both sides' lines but can duplicate or interleave them); a file rewritten per commit conflicts always. Notes merge by set union of whole lines (`git notes merge -s cat_sort_uniq`), which `git evidence fetch` and `push` run.
- **Candidates have nowhere to put their numbers.** A candidate's measurement committed on its branch either lands as part of the change (so review sees data churn, and the numbers describe a tree that differs by the numbers themselves) or is lost with the branch.
- **Nothing checks what a line claims.** In an appended file, each line names its commit or tree by hand, and nothing verifies it, or that the line's numbers mean what the previous line's did. A rewritten file is worse: its history is the database, and "this benchmark over 30 landings" means walking `git log -p`. Either way, `git blame`, `bisect` and every diff carry the churn, and every checkout carries the data.
- **Gaps are invisible.** A missing line is just missing; there is no open run, no refusal with a reason.

The closest alternative is a separate data branch: an orphan branch holding one file per tree id, written by the runner. It avoids the first, second and fourth points. Git notes are that idea built into Git: data about an object, kept on its own refs and keyed by the object's id, attachable after the fact to any commit or tree (including one already landed), never checked out, never in a diff, merged by a standard strategy that cannot conflict, and with no branch to protect or check out. Either way, the storage is the small part. Most of git-evidence is what sits on top: the run grammar that `record` enforces, refusals with reasons, variants that keep instruments apart, and the index that makes the rows queryable.

Notes have costs too. `git fetch` and `git push` do not carry them by default, which is why git-evidence has its own `fetch` and `push`. Hosting sites mostly do not display them. Branch protection rules usually do not cover `refs/notes/*`, so who may push evidence is a matter of who may push to the repository. Shallow and partial clones lack the history and blobs the index needs, so git-evidence refuses them.

**When a file is right.** Some numbers are a function of the code alone: a binary's size, an instruction count from a deterministic simulator, a generated table's row count. Those can be computed by the gate and committed with the change, reviewed in its diff, and checked for freshness like any generated file. git-evidence is for the other kind: observations of running code on a particular machine at a particular time, which are noisy, late, plural and never a property of the tree alone.

## Why it is shaped this way

Each mechanism answers one of the forces above. If a project does not have the force, it can ignore the mechanism.

| Force | Mechanism |
| ----- | --------- |
| A candidate's commit id does not survive landing; unchanged code should not be measured twice | Runs are keyed by **tree**, not commit |
| "Not measured" must not look like "fine"; a crashed producer must leave a trace | A run is **opened before the work**; `refused` and `skipped` outcomes carry reasons; an open run gives no values |
| A runner must keep no state of its own and resume after a crash | Recording is **idempotent** and the store answers "what is left to do" in **SQL** |
| Several machines and clones write concurrently, with no server | **Git notes**, one ref per kind, merged by **set union**; lines are immutable, so nothing conflicts |
| Different harnesses, parameters and machines must never be compared | The index derives a **variant**; `diff` matches only within one; units are declared and never converted |
| Different producers name and scale their numbers differently | Each **metric is declared once per run**, with its UCUM unit and direction |
| Flaky checks and check times matter, not just pass or fail | A verdict keeps its **tries**, each with **wall and CPU time** |
| A producer knows things the tool should not interpret | Free-JSON **close conditions**, exposed in the `conditions` view |
| What counts as a regression differs per project and changes over time | **No verdicts**: rows out, policy as the project's own queries |
| Pushed evidence is in every clone forever | `push` is **opt-in** and **audits** what it sends for personal text (host names, logins, home paths) |
| Many processes and agents ask at once, some running a binary built from a candidate | A disposable **per-build index** with locks and a read time limit |
| Contributors work in jj workspaces as well as Git checkouts | Store discovery and revisions understand **jj workspaces** |
| A runner on a laptop can lose power mid-write | `record` returns only once the objects and the ref are **durable**; a batch is all or nothing |

## When it is not the right tool

- You want a dashboard service with alerting out of the box: git-evidence is a store and a query surface; the dashboard and the alarm are yours to write.
- Your numbers are not about a version of the code (production traffic, business metrics).
- You need raw outputs (logs, profiles, traces) kept: git-evidence keeps numbers and outcomes; a run's `source` and conditions can name where the raw output lives.
