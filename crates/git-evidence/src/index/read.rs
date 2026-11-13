//! Reading typed columns from the index: one column of a query as text, and a
//! stored text parsed into its type, a parse failure reported as SQLite's own
//! conversion error so it travels with the query's. Every reader of the
//! index's tables (the questions) reads through these.

use crate::format::event::{End, Reason, Timestamp};
use crate::format::id::Machine;
use crate::index::IndexError;
use rusqlite::Connection;

pub(crate) fn parsed<T, E: std::error::Error + Send + Sync + 'static>(
    column: usize,
    result: Result<T, E>,
) -> rusqlite::Result<T> {
    result.map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// A run's machine at `column`: `None` where the run names none (NULL).
pub(crate) fn machine_at(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<Machine>> {
    row.get::<_, Option<String>>(column)?
        .map(|name| parsed(column, Machine::parse(&name)))
        .transpose()
}

/// A time at `column`: `None` where the index holds none (NULL).
pub(crate) fn time_at(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<Timestamp>> {
    row.get::<_, Option<String>>(column)?
        .map(|text| parsed(column, Timestamp::parse(&text)))
        .transpose()
}

/// One column of a parameterised query.
pub(crate) fn column_values_with(
    db: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<String>, IndexError> {
    Ok(db
        .prepare(sql)?
        .query_map(params, |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

pub(crate) fn column_values(db: &Connection, sql: &str) -> Result<Vec<String>, IndexError> {
    Ok(db
        .prepare(sql)?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

/// A tag the index stores as its serde spelling (`pass`, `lower`, `derived`)
/// at `column`, parsed back into its enum.
pub(crate) fn tag_at<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(column)?;
    parsed(
        column,
        serde_json::from_value(serde_json::Value::String(text)),
    )
}

/// How a run ended, from its `ended`, `reason` and `detail` columns starting
/// at `column`; `None` while it is open.
pub(crate) fn end_at(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<End>> {
    let ended: Option<String> = row.get(column)?;
    Ok(match ended.as_deref() {
        None => None,
        Some("complete") => Some(End::Complete),
        Some("refused") => Some(End::Refused(Reason {
            reason: row.get(column + 1)?,
            detail: row.get(column + 2)?,
        })),
        Some(other) => return Err(unreadable(column, format!("no run ends {other:?}"))),
    })
}

/// A stored text the index never writes, at `column`: a conversion failure,
/// like a text that does not parse.
pub(crate) fn unreadable(column: usize, why: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, why.into())
}
