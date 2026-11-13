//! The trunk's landings: its first-parent commits, numbered from `#0`, the
//! oldest.

use crate::index::{IndexError, parsed};
use crate::{CommitId, TreeId};
use rusqlite::Connection;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::Serialize;
use std::fmt;

/// A landing's position on the trunk's first-parent line: `0` is the oldest,
/// and each landing after it is one more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct LandingNumber(u32);

impl LandingNumber {
    #[must_use]
    pub const fn new(position: u32) -> Self {
        Self(position)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for LandingNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The index stores a landing's number as an `INTEGER`.
impl FromSql for LandingNumber {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let number = value.as_i64()?;
        u32::try_from(number)
            .map(Self)
            .map_err(|_| FromSqlError::OutOfRange(number))
    }
}

impl ToSql for LandingNumber {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(i64::from(self.0)))
    }
}

/// One commit on the trunk, whether or not anything was recorded on its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Landing {
    /// Its position on the trunk: `0` is the oldest landing.
    #[serde(rename = "landing")]
    pub number: LandingNumber,
    pub commit: CommitId,
    pub tree: TreeId,
    /// The commit message's first line.
    pub subject: String,
    /// Committer time, `YYYY-MM-DDTHH:MM:SSZ`.
    pub date: String,
}

/// The landings at `positions` (a JSON array), oldest first.
pub(crate) fn at_positions(db: &Connection, positions: &str) -> Result<Vec<Landing>, IndexError> {
    let mut statement = db.prepare(&format!(
        "SELECT landing, commit_id, tree, subject, {} FROM trunk l
         WHERE landing IN (SELECT value FROM json_each(?1)) ORDER BY landing",
        crate::index::landing_date("l")
    ))?;
    let rows = statement
        .query_map([positions], |row| {
            Ok(Landing {
                number: row.get(0)?,
                commit: parsed(1, CommitId::parse(&row.get::<_, String>(1)?))?,
                tree: parsed(2, TreeId::parse(&row.get::<_, String>(2)?))?,
                subject: row.get(3)?,
                date: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::LandingNumber;
    use rusqlite::Connection;

    #[test]
    fn a_landing_number_is_a_stored_integer_and_never_negative() {
        let db = Connection::open_in_memory().expect("db");
        let back: LandingNumber = db
            .query_row("SELECT ?1", [LandingNumber::new(7)], |r| r.get(0))
            .expect("round trip");
        assert_eq!(
            (back, back.to_string()),
            (LandingNumber::new(7), "7".to_owned())
        );
        let negative = db.query_row("SELECT -1", [], |r| r.get::<_, LandingNumber>(0));
        assert!(negative.is_err(), "{negative:?}");
    }
}
