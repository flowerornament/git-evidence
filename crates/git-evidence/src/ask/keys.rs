//! `git evidence keys`: the keys and metrics that have evidence, so a question
//! can name them.

use super::NotRecorded;
use super::suggest::nearest;
use crate::format::event::Direction;
use crate::format::id::Kind;
use crate::index::IndexError;
use crate::index::{column_values, direction_from_text, direction_text, parsed};
use crate::rows::{TidyRows, cell, columns};
use rusqlite::{Connection, params};
use serde::Serialize;

/// One key with evidence, and its metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyInfo {
    pub kind: Kind,
    pub key: String,
    /// The runs that record the key.
    pub runs: i64,
    pub metrics: Vec<RecordedMetric>,
}

/// A metric a key records, with the unit and better direction its runs
/// declared. A metric runs declared two ways (two units, or a direction and
/// none) appears once per declaration: each is what some run recorded, and
/// none is chosen for the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedMetric {
    /// The metric's name, as recorded.
    pub name: String,
    /// The unit its runs declared.
    pub unit: String,
    /// Which way is better; `None` when no run declared a direction.
    pub direction: Option<Direction>,
}

/// What `keys` asks: every key with evidence, or only those of one kind, or
/// only metrics matching a glob.
#[derive(Debug, Clone, Default)]
pub struct KeysQuery {
    pub kind: Option<Kind>,
    /// A glob over metric names (`block.*`).
    pub metric: Option<String>,
}

/// `git evidence keys`: the keys and metrics that have evidence.
///
/// # Errors
/// [`NotRecorded::Kind`] for a kind with no run, [`NotRecorded::MetricGlob`]
/// when no recorded metric matches the glob; an index failure.
pub(crate) fn keys(db: &Connection, query: &KeysQuery) -> Result<Vec<KeyInfo>, crate::Error> {
    let (kind, metric) = (query.kind.as_ref(), query.metric.as_deref());
    let mut metrics: std::collections::HashMap<(String, String), Vec<RecordedMetric>> =
        std::collections::HashMap::new();
    let mut statement = db.prepare(
        "SELECT DISTINCT r.kind, m.key, m.metric, m.unit, m.better
         FROM all_measures m JOIN counted_runs r ON r.run = m.run
         WHERE (?1 IS NULL OR r.kind = ?1) AND (?2 IS NULL OR m.metric GLOB ?2)
         ORDER BY m.metric, m.unit, m.better",
    )?;
    let recorded = statement.query_map(params![kind.map(Kind::as_str), metric], |row| {
        Ok((
            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
            RecordedMetric {
                name: row.get(2)?,
                unit: row.get(3)?,
                direction: row
                    .get::<_, Option<String>>(4)?
                    .map(|text| parsed(4, direction_from_text(&text)))
                    .transpose()?,
            },
        ))
    })?;
    for entry in recorded {
        let (at, metric) = entry.map_err(IndexError::from)?;
        metrics.entry(at).or_default().push(metric);
    }
    let rows = db
        .prepare(
            "SELECT r.kind, k.key, COUNT(DISTINCT r.run)
             FROM run_keys k JOIN counted_runs r ON r.run = k.run
             WHERE (?1 IS NULL OR r.kind = ?1)
             GROUP BY r.kind, k.key ORDER BY r.kind, k.key",
        )?
        .query_map(params![kind.map(Kind::as_str)], |row| {
            let (kind, key): (String, String) = (row.get(0)?, row.get(1)?);
            Ok(KeyInfo {
                kind: parsed(0, Kind::parse(&kind))?,
                metrics: metrics.remove(&(kind, key.clone())).unwrap_or_default(),
                key,
                runs: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(IndexError::from)?;
    let rows: Vec<KeyInfo> = rows
        .into_iter()
        .filter(|info| metric.is_none() || !info.metrics.is_empty())
        .collect();
    if let (true, Some(kind)) = (rows.is_empty(), kind) {
        let kinds = column_values(db, "SELECT DISTINCT kind FROM counted_runs ORDER BY kind")?;
        if !kinds.iter().any(|k| k == kind.as_str()) {
            return Err(NotRecorded::Kind {
                kind: kind.to_string(),
                nearest: nearest(kind.as_str(), &kinds),
            }
            .into());
        }
    }
    if let (true, Some(glob)) = (rows.is_empty(), metric) {
        let names = column_values(
            db,
            "SELECT DISTINCT m.metric FROM all_measures m JOIN counted_runs r ON r.run = m.run",
        )?;
        return Err(NotRecorded::MetricGlob {
            glob: glob.to_owned(),
            nearest: nearest(glob, &names),
        }
        .into());
    }
    Ok(rows)
}

/// One row per key and metric, with the metric's unit and direction; a key
/// without metrics has one row, its `metric`, `unit` and `direction` empty.
impl TidyRows for Vec<KeyInfo> {
    fn header(&self) -> Vec<String> {
        columns(&["kind", "key", "runs", "metric", "unit", "direction"])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        let mut rows = Vec::new();
        for k in self {
            let head = || vec![cell(&k.kind), cell(&k.key), cell(k.runs)];
            if k.metrics.is_empty() {
                rows.push([head(), vec![None, None, None]].concat());
            }
            rows.extend(k.metrics.iter().map(|m| {
                [
                    head(),
                    vec![
                        cell(&m.name),
                        cell(&m.unit),
                        m.direction
                            .map(|direction| cell(direction_text(direction)))
                            .unwrap_or_default(),
                    ],
                ]
                .concat()
            }));
        }
        rows
    }
}
