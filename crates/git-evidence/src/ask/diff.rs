//! `git evidence diff A B`: the numbers that differ between two trees, each
//! beside its counterpart. Numbers are matched on kind, machine, variant, key,
//! metric and unit, and the row names its variant, so two series never look
//! alike. Where one side has several complete runs of a series, the newest
//! gives the number, and the row says how many there were and which it took:
//! nothing is chosen silently. A number on one side only differs, and keeps
//! the other side empty; one in another unit is another row, never converted.

use super::NotRecorded;
use super::suggest::{nearest, unknown_key_glob};
use crate::format::id::{Kind, Machine, RunId, Sha256, TreeId};
use crate::index::{IndexError, column_values, machine_at, parsed};
use crate::rows::{TidyRows, cell, columns};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::BTreeMap;

/// What `diff` asks: two trees, and optional filters.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DiffQuery {
    pub before: TreeId,
    pub after: TreeId,
    /// Globs (`SQLite` GLOB) over each run's kind and machine and each
    /// number's key and metric.
    pub kind: Option<String>,
    pub key: Option<String>,
    pub metric: Option<String>,
    pub machine: Option<String>,
}

impl DiffQuery {
    /// Every number that differs between `before` and `after`; narrow it
    /// through its builders.
    #[must_use]
    pub const fn new(before: TreeId, after: TreeId) -> Self {
        Self {
            before,
            after,
            kind: None,
            key: None,
            metric: None,
            machine: None,
        }
    }

    /// Only the runs whose kind is matching the glob `kind` (`SQLite` GLOB; a name
    /// without `*`, `?` or `[` matches only itself).
    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Only the numbers whose key is matching the glob `key` (`SQLite` GLOB; a name
    /// without `*`, `?` or `[` matches only itself).
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Only the numbers whose metric is matching the glob `metric` (`SQLite` GLOB; a name
    /// without `*`, `?` or `[` matches only itself).
    #[must_use]
    pub fn metric(mut self, metric: impl Into<String>) -> Self {
        self.metric = Some(metric.into());
        self
    }

    /// Only the runs whose machine is matching the glob `machine` (`SQLite` GLOB; a name
    /// without `*`, `?` or `[` matches only itself).
    #[must_use]
    pub fn machine(mut self, machine: impl Into<String>) -> Self {
        self.machine = Some(machine.into());
        self
    }
}

/// One number that differs between the two sides; a side without it is
/// `None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Difference {
    pub kind: Kind,
    pub machine: Option<Machine>,
    /// The digest of the runs' kind, machine and parameters.
    pub variant: Sha256,
    pub key: String,
    pub metric: String,
    pub before: Option<RunValue>,
    pub after: Option<RunValue>,
}

/// A recorded number: the newest complete run's, and how many complete runs
/// of the series on that tree recorded it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RunValue {
    pub value: f64,
    pub unit: String,
    pub run: RunId,
    pub runs: u32,
}

/// How far `after` is from `before`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Change {
    /// A fraction of `before`: the unit measures an amount (time, bytes, a count).
    Relative(f64),
    /// In the unit: where `before` is zero, or the unit is already a ratio
    /// (`1`) or a percentage (`%`), so a fraction of it would mislead.
    Absolute(f64),
}

impl Difference {
    /// Whether the two sides differ: in value, or one side lacks it.
    #[must_use]
    pub fn differs(&self) -> bool {
        match (&self.before, &self.after) {
            (Some(before), Some(after)) => before.value.to_bits() != after.value.to_bits(),
            _ => true,
        }
    }

    /// The variant's first 8 digest characters: enough to tell two series of
    /// one key apart; `show RUN` prints a run's parameters.
    #[must_use]
    pub fn short_variant(&self) -> &str {
        let variant = self.variant.as_str();
        let digest = variant.split_once(':').map_or(variant, |(_, d)| d);
        digest.get(..8).unwrap_or(digest)
    }

    /// The change from `before` to `after`; `None` unless both sides hold a
    /// number.
    #[must_use]
    pub fn change(&self) -> Option<Change> {
        let (before, after) = (self.before.as_ref()?, self.after.as_ref()?);
        debug_assert_eq!(before.unit, after.unit, "a row is one unit");
        let difference = after.value - before.value;
        Some(
            if before.value == 0.0 || matches!(before.unit.as_str(), "1" | "%") {
                Change::Absolute(difference)
            } else {
                Change::Relative(difference / before.value)
            },
        )
    }
}

/// Every number within the filters that differs between the two trees, in
/// kind, machine, key and metric order: in value, in unit, or present on one
/// side only.
///
/// # Errors
/// [`NotRecorded`] for a tree with no run, a key nothing records, or a
/// metric glob nothing matches; an index failure.
pub(crate) fn diff(db: &Connection, query: &DiffQuery) -> Result<Vec<Difference>, crate::Error> {
    for tree in [&query.before, &query.after] {
        let any: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM counted_runs WHERE tree = ?1)",
            [tree.as_str()],
            |r| r.get(0),
        )?;
        if !any {
            return Err(NotRecorded::Tree(tree.clone()).into());
        }
    }
    if let Some(glob) = &query.key
        && let Some(unknown) = unknown_key_glob(db, glob)?
    {
        return Err(unknown.into());
    }
    let mut rows: BTreeMap<Series, Difference> = BTreeMap::new();
    for (tree, before) in [(&query.before, true), (&query.after, false)] {
        for (comparison, value) in newest_values(db, query, tree)? {
            let at = (
                comparison.kind.to_string(),
                comparison.machine.as_ref().map(ToString::to_string),
                comparison.key.clone(),
                comparison.metric.clone(),
                comparison.variant.to_string(),
                value.unit.clone(),
            );
            let row = rows.entry(at).or_insert(comparison);
            if before {
                row.before = Some(value);
            } else {
                row.after = Some(value);
            }
        }
    }
    if rows.is_empty()
        && let Some(glob) = &query.metric
    {
        let matched: bool = db.query_row(
            "SELECT EXISTS (SELECT 1 FROM all_measures WHERE metric GLOB ?1)",
            [glob],
            |r| r.get(0),
        )?;
        if !matched {
            let names = column_values(db, "SELECT DISTINCT metric FROM all_measures")?;
            return Err(NotRecorded::MetricGlob {
                glob: glob.clone(),
                nearest: nearest(glob, &names),
            }
            .into());
        }
    }
    Ok(rows.into_values().filter(Difference::differs).collect())
}

/// What a row matches on, in its print order: kind, machine, key, metric,
/// variant, unit.
type Series = (String, Option<String>, String, String, String, String);

/// Each number on `tree` within the filters, from the newest complete run of
/// its series (variant and unit) that recorded it, with the count of such
/// runs.
fn newest_values(
    db: &Connection,
    query: &DiffQuery,
    tree: &TreeId,
) -> Result<Vec<(Difference, RunValue)>, IndexError> {
    let mut statement = db.prepare(
        "SELECT kind, machine, variant, key, metric, value, unit, run, runs FROM (
           SELECT r.kind, r.machine, r.variant, m.key, m.metric, m.value, m.unit, r.run,
                  ROW_NUMBER() OVER series AS newest, COUNT(*) OVER series AS runs
           FROM counted_runs r JOIN all_measures m ON m.run = r.run
           WHERE r.tree = ?1 AND r.ended = 'complete'
             AND (?2 IS NULL OR r.kind GLOB ?2) AND (?3 IS NULL OR m.key GLOB ?3)
             AND (?4 IS NULL OR m.metric GLOB ?4) AND (?5 IS NULL OR r.machine GLOB ?5)
           WINDOW series AS (PARTITION BY r.kind, r.variant, m.key, m.metric, m.unit
                             ORDER BY r.opened DESC, r.run DESC
                             ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING))
         WHERE newest = 1",
    )?;
    let rows = statement
        .query_map(
            params![
                tree.as_str(),
                query.kind,
                query.key,
                query.metric,
                query.machine
            ],
            |row| {
                Ok((
                    Difference {
                        kind: parsed(0, Kind::parse(&row.get::<_, String>(0)?))?,
                        machine: machine_at(row, 1)?,
                        variant: parsed(2, Sha256::parse(&row.get::<_, String>(2)?))?,
                        key: row.get(3)?,
                        metric: row.get(4)?,
                        before: None,
                        after: None,
                    },
                    RunValue {
                        value: row.get(5)?,
                        unit: row.get(6)?,
                        run: parsed(7, RunId::parse(&row.get::<_, String>(7)?))?,
                        runs: row.get(8)?,
                    },
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One row per number that differs between the two trees: `variant` the
/// first 8 characters of the series' digest; either side empty where that
/// tree lacks it; `change` a percentage of `before`, or a difference in the
/// unit ([`Change`]); `before_runs` and `after_runs` how many complete runs of
/// the series recorded it, the newest of which (`before_run`, `after_run`)
/// gave the value.
impl TidyRows for Vec<Difference> {
    fn header(&self) -> Vec<String> {
        columns(&[
            "kind",
            "machine",
            "variant",
            "key",
            "metric",
            "before",
            "after",
            "change",
            "unit",
            "before_runs",
            "after_runs",
            "before_run",
            "after_run",
        ])
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.iter()
            .map(|c| {
                let change = c.change().map(|change| match change {
                    Change::Relative(fraction) => format!("{:+.1}%", fraction * 100.0),
                    Change::Absolute(difference) => format!("{difference:+}"),
                });
                let unit = c.before.as_ref().or(c.after.as_ref()).map(|v| &v.unit);
                let side = |v: &Option<crate::ask::RunValue>| {
                    v.as_ref()
                        .map(|v| (v.value.to_string(), v.runs.to_string(), v.run.to_string()))
                };
                let (before, after) = (side(&c.before), side(&c.after));
                vec![
                    cell(&c.kind),
                    c.machine.as_ref().map(ToString::to_string),
                    cell(c.short_variant()),
                    cell(&c.key),
                    cell(&c.metric),
                    before.as_ref().map(|b| b.0.clone()),
                    after.as_ref().map(|a| a.0.clone()),
                    change,
                    unit.map(ToString::to_string),
                    before.as_ref().map(|b| b.1.clone()),
                    after.as_ref().map(|a| a.1.clone()),
                    before.map(|b| b.2),
                    after.map(|a| a.2),
                ]
            })
            .collect()
    }
}
