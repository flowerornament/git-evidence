//! `git evidence log`: every run on a tree, or on each selected landing's tree,
//! with how it ended and how many keys each outcome has.

use super::NotRecorded;
use super::landings::LandingNumber;
use super::selector::{Selector, json, landings};
use crate::format::event::{End, Timestamp};
use crate::format::id::{Kind, Machine, ProducerName, RunId, TreeId};
use crate::index::IndexError;
use crate::index::{end_at, machine_at, parsed};
use crate::rows::{TidyRows, cell, columns};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;

/// One run on a tree, summarised.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct LogEntry {
    pub run: RunId,
    pub kind: Kind,
    pub producer: ProducerName,
    /// `None` for a run that names no machine.
    pub machine: Option<Machine>,
    /// The landing whose tree the run is on: the selected landing for `log
    /// A..B`, else the first landing with that tree; `None` off the trunk.
    pub landing: Option<LandingNumber>,
    pub opened: Timestamp,
    /// How the run ended; `None` while it is open.
    pub end: Option<End>,
    /// Outcome → number of keys.
    pub outcomes: BTreeMap<String, i64>,
}

/// What `log` asks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum LogQuery {
    /// `git evidence log REV`: every run on this tree.
    Tree(TreeId),
    /// `git evidence log A..B` (or `--since`, `--until`, `--last`): every
    /// run on each selected landing's tree, oldest landing first. A tree two
    /// landings share is listed under each.
    Landings(Selector),
}

/// `git evidence log`.
///
/// # Errors
/// [`NotRecorded::Tree`] when a tree asked for has no run; an index failure.
pub(crate) fn log(db: &Connection, query: &LogQuery) -> Result<Vec<LogEntry>, crate::Error> {
    match query {
        LogQuery::Tree(tree) => {
            let entries = log_entries(
                db,
                "SELECT r.run, r.kind, r.producer, r.machine, r.opened,
                        (SELECT MIN(l.landing) FROM trunk l WHERE l.tree = r.tree),
                        r.ended, r.reason, r.detail
                 FROM counted_runs r WHERE r.tree = ?1 ORDER BY r.kind, r.opened, r.run",
                tree.as_str(),
            )?;
            if entries.is_empty() {
                return Err(NotRecorded::Tree(tree.clone()).into());
            }
            Ok(entries)
        }
        LogQuery::Landings(selector) => Ok(log_entries(
            db,
            "SELECT r.run, r.kind, r.producer, r.machine, r.opened,
                    l.landing, r.ended, r.reason, r.detail
             FROM trunk l JOIN counted_runs r ON r.tree = l.tree
             WHERE l.landing IN (SELECT value FROM json_each(?1))
             ORDER BY l.landing, r.kind, r.opened, r.run",
            &json(&landings(db, selector)?),
        )?),
    }
}

/// The entries `sql` (bound to `param`) lists, each with its outcome counts.
fn log_entries(db: &Connection, sql: &str, param: &str) -> Result<Vec<LogEntry>, IndexError> {
    let entries = db
        .prepare(sql)?
        .query_map([param], |row| {
            Ok(LogEntry {
                run: parsed(0, RunId::parse(&row.get::<_, String>(0)?))?,
                kind: parsed(1, Kind::parse(&row.get::<_, String>(1)?))?,
                producer: parsed(2, ProducerName::parse(&row.get::<_, String>(2)?))?,
                machine: machine_at(row, 3)?,
                landing: row.get(5)?,
                opened: parsed(4, Timestamp::parse(&row.get::<_, String>(4)?))?,
                end: end_at(row, 6)?,
                outcomes: BTreeMap::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut count =
        db.prepare("SELECT outcome, COUNT(*) FROM results WHERE run = ?1 GROUP BY outcome")?;
    let mut detailed = Vec::new();
    for mut entry in entries {
        entry.outcomes = count
            .query_map([entry.run.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        detailed.push(entry);
    }
    Ok(detailed)
}

/// One row per run, with the landing it is listed under (empty off the
/// trunk); the last four columns count its keys of each outcome.
impl TidyRows for Vec<LogEntry> {
    fn header(&self) -> Vec<String> {
        columns(&[
            "landing", "run", "kind", "producer", "machine", "opened", "ended", "measured",
            "verdict", "refused", "skipped",
        ])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.iter()
            .map(|e| {
                let count = |outcome: &str| cell(e.outcomes.get(outcome).copied().unwrap_or(0));
                vec![
                    e.landing.map(|l| l.to_string()),
                    cell(&e.run),
                    cell(&e.kind),
                    cell(&e.producer),
                    e.machine.as_ref().map(ToString::to_string),
                    cell(&e.opened),
                    e.end.as_ref().map(ended),
                    count("measured"),
                    count("verdict"),
                    count("refused"),
                    count("skipped"),
                ]
            })
            .collect()
    }
}

/// How a run ended, as `log` prints it: `complete`, or `refused: <reason>`.
fn ended(end: &End) -> String {
    match end {
        End::Complete => "complete".to_owned(),
        End::Refused(reason) => format!("refused: {}", reason.reason),
    }
}
