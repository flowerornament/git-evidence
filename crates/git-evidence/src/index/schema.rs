//! The index's tables and their state: the schema, and preparing a file for
//! this build's source. The tables are
//! the index's own and change with [`VIEWS_VERSION`]; the public views over
//! them are [`crate::ask::sql`]'s.

use super::handle::IndexError;
use super::per_build::SOURCE;
use rusqlite::{Connection, OptionalExtension, params};

/// The views' version: part of every pin, so an upgrade that changes what a
/// question means cannot silently change an answer at the same tips.
pub const VIEWS_VERSION: u32 = 31;

/// SQL for the date of the landing in `trunk` row `alias`: its committer
/// time as `YYYY-MM-DDTHH:MM:SSZ`, the one spelling of a landing's date.
pub(crate) fn landing_date(alias: &str) -> String {
    format!("strftime('%Y-%m-%dT%H:%M:%SZ', {alias}.committed, 'unixepoch')")
}

/// The tables that hold rows of one run, besides `all_runs` itself: a run's
/// rows are removed from each when the run is, and only then from `all_runs`.
pub(super) const RUN_ROWS: [&str; 5] = [
    "results",
    "all_measures",
    "run_keys",
    "run_params",
    "run_conditions",
];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- The tips the index was folded at: `trunk:<ref>` (no tip when the ref does
-- not resolve) and `notes:<kind>`.
CREATE TABLE IF NOT EXISTS pin (ref TEXT PRIMARY KEY, tip TEXT);
-- The landings: the trunk's first-parent line, oldest first, each with its
-- committer time in Unix seconds.
CREATE TABLE IF NOT EXISTS trunk (landing INTEGER PRIMARY KEY, commit_id TEXT UNIQUE NOT NULL,
  tree TEXT NOT NULL, committed INTEGER NOT NULL, subject TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS trunk_tree ON trunk (tree);
CREATE TABLE IF NOT EXISTS all_runs (run TEXT PRIMARY KEY, kind TEXT NOT NULL, tree TEXT NOT NULL,
  commit_id TEXT NOT NULL, producer TEXT NOT NULL, variant TEXT NOT NULL, machine TEXT,
  opened TEXT NOT NULL, closed TEXT, ended TEXT, reason TEXT, detail TEXT, open TEXT NOT NULL,
  conditions TEXT);
CREATE INDEX IF NOT EXISTS runs_tree ON all_runs (kind, tree);
CREATE TABLE IF NOT EXISTS run_keys (run TEXT NOT NULL, key TEXT NOT NULL, PRIMARY KEY (run, key));
CREATE TABLE IF NOT EXISTS run_params (run TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
  PRIMARY KEY (run, name, value));
-- Each scalar of a run's close conditions by its dotted path
-- (`rows::condition_leaves`), every leaf `show RUN` prints: two names can
-- spell one path (`a.b` and `a` holding `b`), so a path is not a key.
CREATE TABLE IF NOT EXISTS run_conditions (run TEXT NOT NULL, path TEXT NOT NULL, value TEXT);
CREATE INDEX IF NOT EXISTS run_conditions_by_run ON run_conditions (run);
-- Each key's outcome; a verdict's number of tries in `tries`.
CREATE TABLE IF NOT EXISTS results (run TEXT NOT NULL, key TEXT NOT NULL, outcome TEXT NOT NULL,
  reason TEXT, detail TEXT, status TEXT, tries INTEGER, location TEXT, digest TEXT NOT NULL,
  PRIMARY KEY (run, key));
-- Each measure with its run's declaration of the metric; origin 'recorded' or
-- 'derived' (verdict durations, derived here and never stored in notes).
CREATE TABLE IF NOT EXISTS all_measures (run TEXT NOT NULL, key TEXT NOT NULL, metric TEXT NOT NULL,
  value REAL NOT NULL, unit TEXT NOT NULL, better TEXT, n INTEGER, mad REAL,
  origin TEXT NOT NULL, PRIMARY KEY (run, key, metric));
-- Answers ask by tree; by key and metric, LOOKUPS below.
CREATE INDEX IF NOT EXISTS runs_by_tree ON all_runs (tree);
-- cause: 'fold' (the fold reported it) or 'copies' (one run on several trees).
CREATE TABLE IF NOT EXISTS violations (kind TEXT NOT NULL, tree TEXT NOT NULL, run TEXT,
  problem TEXT NOT NULL, cause TEXT NOT NULL);
-- The runs every answer reads: all folded runs except those whose commit is a
-- landing with another tree. Derived, so it follows the trunk.
CREATE VIEW IF NOT EXISTS counted_runs AS
  SELECT r.* FROM all_runs r WHERE NOT EXISTS
    (SELECT 1 FROM trunk l WHERE l.commit_id = r.commit_id AND l.tree != r.tree);
-- Every problem with recorded evidence: what was stored, and the misfiled runs.
CREATE VIEW IF NOT EXISTS all_violations AS
  SELECT kind, tree, run, problem FROM violations
  UNION
  SELECT r.kind, r.tree, r.run, 'opened for commit ' || r.commit_id || ' but filed on another tree'
  FROM all_runs r JOIN trunk l ON l.commit_id = r.commit_id WHERE l.tree != r.tree;

-- Every tree each run id was seen on. A run seen on more than one is excluded
-- everywhere, whichever note was folded first.
CREATE TABLE IF NOT EXISTS sightings (run TEXT NOT NULL, kind TEXT NOT NULL, tree TEXT NOT NULL,
  PRIMARY KEY (run, kind, tree));
";

/// The indexes answers search by that no fold reads, created once the fold
/// is in: a cold fold inserts every measure, and keeping this index ordered
/// row by row wrote most of the file twice. Every catch-up runs this; on a
/// caught-up file it does nothing.
pub(super) const LOOKUPS: &str = "
-- Answers ask by key and metric (trend, diff).
CREATE INDEX IF NOT EXISTS measures_key_metric ON all_measures (key, metric);
";

/// The value `meta` holds under `key`: `None` when the key, or the table (a
/// new file), is absent.
fn meta(db: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let table: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |row| row.get(0),
    )?;
    if !table {
        return Ok(None);
    }
    db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
}

/// Whether this build prepared `db`: its tables are of this views version,
/// and everything in it was made by binaries built from this source.
pub(super) fn prepared(db: &Connection) -> rusqlite::Result<bool> {
    Ok(
        meta(db, "views")?.as_deref() == Some(VIEWS_VERSION.to_string().as_str())
            && meta(db, "source")?.as_deref() == Some(SOURCE),
    )
}

/// Makes `db` this build's, in the caller's write transaction: every table
/// and view is dropped and recreated empty, so the catch-up that follows
/// folds every note by this build's code. Does nothing when another process prepared it meanwhile.
pub(super) fn prepare(db: &Connection) -> Result<(), IndexError> {
    if prepared(db)? {
        return Ok(());
    }
    drop_all(db)?;
    db.execute_batch(SCHEMA)?;
    db.execute_batch(&crate::ask::sql::views())?;
    db.pragma_update(None, "user_version", crate::ask::sql::PUBLIC_SCHEMA)?;
    for (key, value) in [
        ("views", VIEWS_VERSION.to_string()),
        ("source", SOURCE.to_owned()),
    ] {
        execute_cached(
            db,
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    Ok(())
}

/// A count or a landing's position as the integer SQLite stores: one too
/// large to store is the write's error, never a stand-in value.
pub(super) fn sql_integer(n: usize) -> rusqlite::Result<i64> {
    i64::try_from(n).map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

/// Runs one statement through the connection's statement cache: a fold
/// executes the same few statements once per row, and preparing each anew
/// cost as much as the insert.
pub(super) fn execute_cached(
    db: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<usize> {
    db.prepare_cached(sql)?.execute(params)
}

/// Drops every table and view.
fn drop_all(db: &Connection) -> rusqlite::Result<()> {
    let objects: Vec<(String, String)> = db
        .prepare("SELECT type, name FROM sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (kind, name) in objects {
        let kind = if kind == "view" { "VIEW" } else { "TABLE" };
        db.execute_batch(&format!("DROP {kind} IF EXISTS \"{name}\";"))?;
    }
    Ok(())
}
