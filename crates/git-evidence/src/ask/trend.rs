//! `git evidence trend METRIC KEY`: one recorded value per run, over the
//! selected landings, oldest first. Several runs on one landing are several
//! rows; nothing is averaged, chosen or compared. Each row names its run's
//! kind and variant, so values that `diff` would never compare (another
//! instrument, other parameters) never look like one series.

use super::landings::LandingNumber;
use super::selector::{Selector, json, landings};
use super::suggest::unknown_name;
use crate::format::id::{CommitId, Key, Kind, Machine, MetricName, RunId, Sha256};
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
    /// The run's kind.
    pub kind: Kind,
    /// `None` for a run that names no machine.
    pub machine: Option<Machine>,
    /// The digest of what must match for two values to be compared: the
    /// run's kind, producer, harness, parameters, build profile and machine.
    pub variant: Sha256,
    pub value: f64,
    /// The unit the run declared for the metric.
    pub unit: String,
    pub run: RunId,
}

impl Point {
    /// The variant's first 8 digest characters, as the row prints it.
    #[must_use]
    pub fn short_variant(&self) -> &str {
        super::short_variant(&self.variant)
    }
}

/// What `trend` asks.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TrendQuery {
    pub metric: MetricName,
    pub key: Key,
    /// A glob (`SQLite` GLOB) over each run's kind.
    pub kind: Option<String>,
    /// A glob (`SQLite` GLOB) over each run's machine.
    pub machine: Option<String>,
    /// The start of one variant's digest (the 8 characters a row prints, or
    /// more): only that variant's runs.
    pub variant: Option<String>,
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
            kind: None,
            machine: None,
            variant: None,
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

    /// Only runs whose kind matches `kind`, a glob.
    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Only the runs of the variant whose digest starts with `prefix` (the
    /// 8 characters a row prints, or more), in either case.
    #[must_use]
    pub fn variant(mut self, prefix: impl Into<String>) -> Self {
        self.variant = Some(prefix.into().to_ascii_lowercase());
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
        "SELECT l.landing, l.commit_id, {}, r.kind, r.machine, r.variant, m.value, m.unit, r.run
         FROM trunk l
         JOIN counted_runs r ON r.tree = l.tree AND r.ended IS NOT NULL
         JOIN all_measures m ON m.run = r.run AND m.key = ?1 AND m.metric = ?2
         WHERE l.landing IN (SELECT value FROM json_each(?3)) AND (?4 IS NULL OR r.machine GLOB ?4)
           AND (?5 IS NULL OR r.kind GLOB ?5)
           AND (?6 IS NULL OR substr(r.variant, instr(r.variant, ':') + 1, length(?6)) = ?6)
         ORDER BY l.landing, r.opened, r.run",
        crate::index::landing_date("l")
    ))?;
    let points = statement
        .query_map(
            params![
                query.key,
                query.metric,
                positions,
                query.machine,
                query.kind,
                query.variant
            ],
            |row| {
                Ok(Point {
                    landing: row.get(0)?,
                    commit: parsed(1, CommitId::parse(&row.get::<_, String>(1)?))?,
                    date: row.get(2)?,
                    kind: parsed(3, Kind::parse(&row.get::<_, String>(3)?))?,
                    machine: machine_at(row, 4)?,
                    variant: parsed(5, Sha256::parse(&row.get::<_, String>(5)?))?,
                    value: row.get(6)?,
                    unit: row.get(7)?,
                    run: parsed(8, RunId::parse(&row.get::<_, String>(8)?))?,
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
            "landing", "commit", "date", "kind", "machine", "variant", "value", "unit", "run",
        ])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.iter()
            .map(|p| {
                vec![
                    cell(p.landing),
                    cell(&p.commit),
                    cell(&p.date),
                    cell(&p.kind),
                    p.machine.as_ref().map(ToString::to_string),
                    cell(p.short_variant()),
                    cell(p.value),
                    cell(&p.unit),
                    cell(&p.run),
                ]
            })
            .collect()
    }
}
