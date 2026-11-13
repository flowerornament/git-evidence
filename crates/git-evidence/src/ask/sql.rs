//! The public SQL surface: documented views over the index, and `sql`, the
//! way to read them.
//!
//! ```text
//!   index (internal tables, free to change)  ──►  public views (schema PUBLIC_SCHEMA)
//!                                                   about · landings · runs · params
//!                                                   conditions · measures · verdicts
//!                                                   outcomes
//!   Evidence::sql(q):  catch up (read-write)  ──►  a read-only connection, one snapshot:
//!                      pin read, q prepared, refused unless it only reads rows
//! ```
//!
//! The views are a contract of their own: their names, columns and column
//! types change only with [`PUBLIC_SCHEMA`], and `tests/public_schema.snap`
//! holds every version's columns. Each column has a declared type, so
//! `PRAGMA table_info` names it: a column read straight from an index table
//! keeps that table's type (and its indexes), and every computed column is
//! `CAST` to its type. The index's own tables are not part of the contract.
//!
//! The views hold recorded numbers and how each run ended (`ended`,
//! `conditions`, `variant`); they judge nothing. Comparing them is the
//! caller's query.

use crate::format::event::DERIVED_PREFIX;
use crate::index::{Cached, Index, IndexError, VIEWS_VERSION, landing_date, pin_in, within};
use rusqlite::{Connection, OpenFlags, types::ValueRef};

/// The public views' version: bumped on any change to their names, columns
/// or column types. Also the index file's `PRAGMA user_version`.
/// Bumping it changes what a `sql` question means, so it also bumps the
/// index's views version, which every pin carries.
pub const PUBLIC_SCHEMA: u32 = 6;

/// The public views, in the order the README documents them.
pub const PUBLIC_VIEWS: [&str; 8] = [
    "about",
    "landings",
    "runs",
    "params",
    "conditions",
    "measures",
    "verdicts",
    "outcomes",
];

/// Views a later schema will add; none are pending.
const PENDING_VIEWS: &str = "";

/// The view of the index file itself, one row: its versions, the trunk it
/// pins, and its views.
fn about() -> String {
    let public = PUBLIC_VIEWS.join(" ");
    format!(
        "
CREATE VIEW IF NOT EXISTS about AS
  SELECT CAST({PUBLIC_SCHEMA} AS INTEGER) AS schema_version,
         CAST({VIEWS_VERSION} AS INTEGER) AS views_version,
         CAST(substr(ref, 7) AS TEXT) AS trunk_ref,
         CAST(tip AS TEXT) AS trunk,
         CAST('{public}' AS TEXT) AS public_views,
         CAST('{PENDING_VIEWS}' AS TEXT) AS pending_views
  FROM pin WHERE ref LIKE 'trunk:%';"
    )
}

/// The statements that create the public views over the index's tables.
///
/// A view over one table filters counted runs with `IN`, never by joining
/// them: SQLite flattens a one-table view into the right side of a LEFT JOIN
/// and reads it by its key, but materializes a joined view whole, once per
/// join. Metrics compared side by side are a chain of LEFT JOINs on
/// `measures`.
pub(crate) fn views() -> String {
    let date = landing_date("trunk");
    let (quiet, disturbed) = ("quiet", "disturbed");
    let about = about();
    format!(
        "{about}
CREATE VIEW IF NOT EXISTS landings AS
  SELECT landing AS landing, commit_id AS commit_id,
         tree AS tree,
         CAST({date} AS TEXT) AS date,
         subject AS subject
  FROM trunk;
CREATE VIEW IF NOT EXISTS runs AS
  SELECT r.run AS run, r.kind AS kind,
         r.producer AS producer, r.machine AS machine,
         r.variant AS variant,
         CAST(CASE WHEN json_extract(r.conditions, '$.disturbed') IS NULL THEN '{quiet}'
                   ELSE '{disturbed}' END AS TEXT) AS conditions,
         CAST(json_extract(r.conditions, '$.disturbed') AS TEXT) AS disturbed,
         CAST((SELECT MIN(l.landing) FROM trunk l WHERE l.tree = r.tree) AS INTEGER) AS landing,
         r.tree AS tree, r.commit_id AS commit_id,
         r.opened AS opened, r.closed AS closed,
         r.ended AS ended, r.reason AS reason
  FROM counted_runs r;
CREATE VIEW IF NOT EXISTS params AS
  SELECT p.run AS run, p.name AS name,
         p.value AS value
  FROM run_params p WHERE p.run IN (SELECT run FROM counted_runs);
CREATE VIEW IF NOT EXISTS conditions AS
  SELECT c.run AS run, c.path AS path,
         c.value AS value
  FROM run_conditions c WHERE c.run IN (SELECT run FROM counted_runs);
CREATE VIEW IF NOT EXISTS measures AS
  SELECT q.run AS run, q.key AS key,
         q.metric AS metric, q.value AS value,
         q.unit AS unit, q.better AS direction,
         q.n AS n, q.mad AS mad,
         q.origin AS origin
  FROM all_measures q WHERE q.run IN (SELECT run FROM counted_runs);
CREATE VIEW IF NOT EXISTS verdicts AS
  SELECT o.run AS run, o.key AS key,
         o.status AS status,
         CAST((SELECT q.value FROM all_measures q WHERE q.run = o.run AND q.key = o.key
                 AND q.metric = '{DERIVED_PREFIX}' || o.status || '.duration_ms') AS REAL)
           AS duration_ms,
         CAST((SELECT q.value FROM all_measures q WHERE q.run = o.run AND q.key = o.key
                 AND q.metric = '{DERIVED_PREFIX}' || o.status || '.cpu_ms') AS REAL) AS cpu_ms,
         o.tries AS tries
  FROM results o WHERE o.outcome = 'verdict' AND o.run IN (SELECT run FROM counted_runs);
CREATE VIEW IF NOT EXISTS outcomes AS
  SELECT o.run AS run, o.key AS key,
         o.outcome AS outcome, o.status AS status,
         o.reason AS reason, o.detail AS detail,
         o.location AS location
  FROM results o WHERE o.run IN (SELECT run FROM counted_runs);
"
    )
}

/// One value of a row, as SQLite returned it. `Null` is a value the row does
/// not hold: an empty field in CSV.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    /// Bytes; lowercase hexadecimal in CSV.
    Blob(Vec<u8>),
}

impl SqlValue {
    /// The value as one CSV or text cell: `None` for `Null`.
    #[must_use]
    pub fn cell(&self) -> Option<String> {
        match self {
            Self::Null => None,
            Self::Integer(value) => Some(value.to_string()),
            Self::Real(value) => Some(value.to_string()),
            Self::Text(text) => Some(text.clone()),
            Self::Blob(bytes) => Some(hex(bytes)),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        // Writing to a String cannot fail.
        write!(out, "{b:02x}").ok();
        out
    })
}

/// The rows one `sql` query returned: its columns, and one value per column
/// in each row.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Rows {
    /// The column names, distinct, in order.
    pub columns: Vec<String>,
    /// One value per column in each row.
    pub rows: Vec<Vec<SqlValue>>,
}

/// Why `sql` did not run a statement.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SqlError {
    #[error("the statement would write; sql only reads (the index is a cache of the notes)")]
    Writes,
    #[error("the statement returns no rows; sql runs one query (SELECT, WITH, VALUES, PRAGMA)")]
    NoRows,
    #[error("two columns are named {0:?}; name each column once (`AS`)")]
    DuplicateColumn(String),
    #[error("the index kept no pin for trunk {0}: another reader cached another trunk; ask again")]
    NoPin(String),
    #[error(
        "a git-evidence built from other source rebuilt the index since this one caught up; \
         ask again"
    )]
    OtherSource,
    #[error("{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// Catches `index` up, then runs `query` on a second, read-only connection in
/// one snapshot, the pin read in that snapshot, under the index's time limit.
/// The read-write connection stays open meanwhile: a read-only connection
/// cannot open a WAL database whose shared-memory file is gone.
pub(crate) fn query(index: &mut Index, query: &str) -> Result<Rows, SqlError> {
    index.catch_up()?;
    let db = Connection::open_with_flags(
        index.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.pragma_update(None, "query_only", true)?;
    db.busy_timeout(std::time::Duration::from_secs(30))?;
    db.execute_batch("BEGIN DEFERRED")?;
    let rows = within(&db, index.read_limit(), || {
        read(&db, index.trunk_ref(), query)
    });
    // A read transaction holds nothing worth reporting on its way out;
    // ending it releases the snapshot.
    db.execute_batch("COMMIT").ok();
    rows
}

fn read(db: &Connection, trunk_ref: &str, query: &str) -> Result<Rows, SqlError> {
    match pin_in(db, trunk_ref)? {
        Cached::Pin(_) => {}
        Cached::OtherTrunk => return Err(SqlError::NoPin(trunk_ref.to_owned())),
        Cached::OtherSource => return Err(SqlError::OtherSource),
    }
    let mut statement = db.prepare(query)?;
    if !statement.readonly() {
        return Err(SqlError::Writes);
    }
    if statement.column_count() == 0 {
        return Err(SqlError::NoRows);
    }
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    for (i, column) in columns.iter().enumerate() {
        if columns[..i].contains(column) {
            return Err(SqlError::DuplicateColumn(column.clone()));
        }
    }
    let width = columns.len();
    let rows = statement
        .query_map([], |row| {
            (0..width)
                .map(|i| {
                    Ok(match row.get_ref(i)? {
                        ValueRef::Null => SqlValue::Null,
                        ValueRef::Integer(value) => SqlValue::Integer(value),
                        ValueRef::Real(value) => SqlValue::Real(value),
                        ValueRef::Text(text) => {
                            SqlValue::Text(String::from_utf8_lossy(text).into_owned())
                        }
                        ValueRef::Blob(bytes) => SqlValue::Blob(bytes.to_vec()),
                    })
                })
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Rows { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::{SqlError, read};
    use crate::index::fixtures::{TRUNK, another_build_prepares, index};
    use crate::testing::Scratch;
    use rusqlite::{Connection, OpenFlags};

    /// Metrics side by side are a chain of LEFT JOINs on one view; each must
    /// read the view by its own key, never materialize it or build an
    /// automatic index over it (a full scan of every measure per join), as a
    /// view defined by a JOIN does.
    #[test]
    fn a_left_join_on_a_view_reads_it_by_key() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let index = index(&scratch);
        let db = Connection::open_with_flags(index.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only connection");
        for (view, on) in [
            (
                "measures",
                "b.run = a.run AND b.key = a.key AND b.metric = 'other'",
            ),
            ("verdicts", "b.run = a.run AND b.key = a.key"),
            ("outcomes", "b.run = a.run AND b.key = a.key"),
        ] {
            let plan = read(
                &db,
                TRUNK,
                &format!("EXPLAIN QUERY PLAN SELECT * FROM {view} a LEFT JOIN {view} b ON {on}"),
            )
            .expect("plan");
            let details: Vec<String> = plan
                .rows
                .iter()
                .filter_map(|row| row.last().and_then(super::SqlValue::cell))
                .collect();
            assert!(
                details.iter().all(|detail| !detail.contains("MATERIALIZE")),
                "{view}: {details:#?}"
            );
            assert!(
                details.iter().any(|detail| detail.contains("LEFT-JOIN")
                    && detail.contains("USING")
                    && detail.contains("INDEX")
                    && !detail.contains("AUTOMATIC")),
                "{view}: {details:#?}"
            );
        }
    }

    #[test]
    fn sql_refuses_a_snapshot_another_build_prepared_after_its_catch_up() {
        let scratch = Scratch::new()
            .expect("scratch")
            .landings(1)
            .expect("landing");
        let index = index(&scratch);
        let db = Connection::open_with_flags(index.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read-only connection");
        assert!(read(&db, TRUNK, "SELECT 1").is_ok());
        another_build_prepares(index.path());
        let refused = read(&db, TRUNK, "SELECT 1");
        assert!(matches!(refused, Err(SqlError::OtherSource)), "{refused:?}");
    }
}
