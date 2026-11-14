//! `git evidence trend METRIC KEY`: one recorded value per run, over the
//! selected landings, oldest first. Several runs on one landing are several
//! rows; nothing is averaged, chosen or compared.

use super::landings::LandingNumber;
use super::selector::{Selector, json, landings};
use super::suggest::unknown_name;
use crate::format::id::{CommitId, Key, Machine, MetricName, RunId};
use crate::index::{IndexError, machine_at, parsed};
use crate::rows::{TidyRows, cell, columns};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::num::NonZeroU32;

/// One run's value of the metric on one landing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Point {
    pub landing: LandingNumber,
    pub commit: CommitId,
    /// The landing's committer time, `YYYY-MM-DDTHH:MM:SSZ`.
    pub date: String,
    /// `None` for a run that names no machine.
    pub machine: Option<Machine>,
    pub value: f64,
    /// The unit the run declared for the metric.
    pub unit: String,
    pub run: RunId,
}

/// What `trend` asks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TrendQuery {
    pub metric: MetricName,
    pub key: Key,
    /// A glob (`SQLite` GLOB) over each run's machine.
    pub machine: Option<String>,
    /// The landings it spans.
    pub landings: Selector,
}

impl TrendQuery {
    /// How many of the newest landings a trend spans when it names none.
    pub const DEFAULT_LAST: NonZeroU32 = NonZeroU32::new(30).expect("30 is not zero");

    /// `metric` of `key` over the newest [`Self::DEFAULT_LAST`] landings,
    /// on every machine.
    #[must_use]
    pub fn new(metric: MetricName, key: Key) -> Self {
        Self {
            metric,
            key,
            machine: None,
            landings: Selector::newest(Self::DEFAULT_LAST),
        }
    }

    /// Only the runs whose machine is matching the glob `machine` (`SQLite` GLOB; a name
    /// without `*`, `?` or `[` matches only itself).
    #[must_use]
    pub fn machine(mut self, machine: impl Into<String>) -> Self {
        self.machine = Some(machine.into());
        self
    }

    /// Over the landings `landings` selects.
    #[must_use]
    pub fn landings(mut self, landings: Selector) -> Self {
        self.landings = landings;
        self
    }
}

/// Every ended run's value of the metric on the selected landings, by
/// landing, then by when the run opened.
///
/// # Errors
/// [`super::NotRecorded`] for a key or metric nothing records; an index
/// failure.
pub(crate) fn trend(db: &Connection, query: &TrendQuery) -> Result<Vec<Point>, crate::Error> {
    if let Some(unknown) = unknown_name(db, query.key.as_str(), query.metric.as_str())? {
        return Err(unknown.into());
    }
    let positions = json(&landings(db, &query.landings)?);
    let mut statement = db.prepare(&format!(
        "SELECT l.landing, l.commit_id, {}, r.machine, m.value, m.unit, r.run
         FROM trunk l
         JOIN counted_runs r ON r.tree = l.tree AND r.ended IS NOT NULL
         JOIN all_measures m ON m.run = r.run AND m.key = ?1 AND m.metric = ?2
         WHERE l.landing IN (SELECT value FROM json_each(?3)) AND (?4 IS NULL OR r.machine GLOB ?4)
         ORDER BY l.landing, r.opened, r.run",
        crate::index::landing_date("l")
    ))?;
    let points = statement
        .query_map(
            params![query.key, query.metric, positions, query.machine],
            |row| {
                Ok(Point {
                    landing: row.get(0)?,
                    commit: parsed(1, CommitId::parse(&row.get::<_, String>(1)?))?,
                    date: row.get(2)?,
                    machine: machine_at(row, 3)?,
                    value: row.get(4)?,
                    unit: row.get(5)?,
                    run: parsed(6, RunId::parse(&row.get::<_, String>(6)?))?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(IndexError::from)?;
    Ok(points)
}

/// One row per run's value, oldest landing first.
impl TidyRows for Vec<Point> {
    fn header(&self) -> Vec<String> {
        columns(&[
            "landing", "commit", "date", "machine", "value", "unit", "run",
        ])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.iter()
            .map(|p| {
                vec![
                    cell(p.landing),
                    cell(&p.commit),
                    cell(&p.date),
                    p.machine.as_ref().map(ToString::to_string),
                    cell(p.value),
                    cell(&p.unit),
                    cell(&p.run),
                ]
            })
            .collect()
    }
}
