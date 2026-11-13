//! `git evidence show REV|RUN`: every run on a revision's tree, or one run,
//! with every outcome recorded, optionally narrowed by globs over kind,
//! machine, key and metric.

use super::NotRecorded;
use super::params_of::params_of;
use super::suggest::nearest;
use crate::format::event::{Direction, End, Open, Reason, Timestamp, VerdictStatus};
use crate::format::id::{Key, Kind, Machine, MetricName, RunId, Sha256, TreeId};
use crate::index::{
    DeclaredMeasure, IndexError, column_values_with, end_at, machine_at, parsed, tag_at, time_at,
    unreadable,
};
use crate::rows::{TidyRows, cell, columns, condition_leaves};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// A run with its outcomes, as the index holds it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct RunDetail {
    pub run: RunId,
    pub kind: Kind,
    pub tree: TreeId,
    /// `None` for a run that names no machine.
    pub machine: Option<Machine>,
    /// What tells this run's numbers apart from another's: a digest of its
    /// kind, machine and parameters.
    pub variant: Sha256,
    /// The run's parameters: its producer, its harness, then the
    /// environment's, the suite's and the build's parameters.
    pub params: Vec<Param>,
    pub opened: Timestamp,
    pub closed: Option<Timestamp>,
    /// How the run ended; `None` while it is open.
    pub end: Option<End>,
    /// The run's open, completed by its declaration (its keys and build).
    pub open: Open,
    /// The conditions the producer recorded at close, as it wrote them: the
    /// format gives them no schema. `None` while open, or when the close
    /// recorded none.
    pub conditions: Option<Value>,
    /// One per key, in key order.
    pub outcomes: Vec<KeyOutcome>,
}

/// One parameter of a run: its name and its value as text (a JSON value
/// other than a string as JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Param {
    pub name: String,
    pub value: String,
}

/// One key of a run: its outcome, and where the producer said it was found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct KeyOutcome {
    pub key: Key,
    pub outcome: ShownOutcome,
    pub location: Option<String>,
}

/// A key's [`Outcome`](crate::Outcome) as the index holds it: each number
/// beside its run's declaration of the metric, and a verdict with the times
/// derived from its tries in place of the tries. A key a run left unanswered
/// when it ended refused holds the end's refusal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShownOutcome {
    Measured(Vec<DeclaredMeasure>),
    Verdict {
        status: VerdictStatus,
        measures: Vec<DeclaredMeasure>,
    },
    Refused(Reason),
    Skipped(Reason),
}

impl ShownOutcome {
    /// The outcome's name as events spell it: `measured`, `verdict`,
    /// `refused` or `skipped`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Measured(_) => "measured",
            Self::Verdict { .. } => "verdict",
            Self::Refused(_) => "refused",
            Self::Skipped(_) => "skipped",
        }
    }

    /// The numbers it holds: recorded, or derived from a verdict's tries.
    #[must_use]
    pub fn measures(&self) -> &[DeclaredMeasure] {
        match self {
            Self::Measured(measures) | Self::Verdict { measures, .. } => measures,
            Self::Refused(_) | Self::Skipped(_) => &[],
        }
    }
}

/// What `show` names: one run, or every run on a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Run(RunId),
    Tree(TreeId),
}

/// What `show` asks: a run or a tree, narrowed by globs (`SQLite` GLOB, a
/// pattern, not a name) over each run's kind and machine and each outcome's
/// key and metric.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ShowQuery {
    pub shown: Shown,
    pub kind: Option<String>,
    pub machine: Option<String>,
    pub key: Option<String>,
    pub metric: Option<String>,
}

impl ShowQuery {
    /// The whole of one run: every key and metric.
    #[must_use]
    pub fn of(run: &RunId) -> Self {
        Self::new(Shown::Run(run.clone()))
    }

    /// Every run on `tree`, every key and metric.
    #[must_use]
    pub fn tree(tree: &TreeId) -> Self {
        Self::new(Shown::Tree(tree.clone()))
    }

    /// Every key and metric of what `shown` names; narrow it through the fields.
    #[must_use]
    pub const fn new(shown: Shown) -> Self {
        Self {
            shown,
            kind: None,
            machine: None,
            key: None,
            metric: None,
        }
    }

    /// Only the runs whose kind matches the glob `kind`.
    #[must_use]
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Only the runs whose machine matches the glob `machine`.
    #[must_use]
    pub fn machine(mut self, machine: impl Into<String>) -> Self {
        self.machine = Some(machine.into());
        self
    }

    /// Only the outcomes whose key matches the glob `key`.
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Only the numbers whose metric matches the glob `metric`.
    #[must_use]
    pub fn metric(mut self, metric: impl Into<String>) -> Self {
        self.metric = Some(metric.into());
        self
    }
}

/// The runs `show` names, oldest first within each kind, each with the
/// outcomes the globs keep; a run left with none is left out when a key or
/// metric glob was given.
///
/// # Errors
/// [`NotRecorded`] for a run or a tree with no run, or a key or metric glob
/// nothing in the store matches; an index failure.
pub(crate) fn show(db: &Connection, query: &ShowQuery) -> Result<Vec<RunDetail>, crate::Error> {
    let ids = match &query.shown {
        Shown::Run(run) => {
            if run_detail(db, run)?.is_none() {
                return Err(missing_run(db, run)?.into());
            }
            vec![run.as_str().to_owned()]
        }
        Shown::Tree(tree) => {
            let ids = column_values_with(
                db,
                "SELECT run FROM counted_runs WHERE tree = ?1 AND (?2 IS NULL OR kind GLOB ?2)
                   AND (?3 IS NULL OR machine GLOB ?3) ORDER BY kind, opened, run",
                params![tree.as_str(), query.kind, query.machine],
            )?;
            let any: bool = db.query_row(
                "SELECT EXISTS (SELECT 1 FROM counted_runs WHERE tree = ?1)",
                [tree.as_str()],
                |r| r.get(0),
            )?;
            if !any {
                return Err(NotRecorded::Tree(tree.clone()).into());
            }
            ids
        }
    };
    let narrowed = query.key.is_some() || query.metric.is_some();
    let mut runs = Vec::new();
    for id in ids {
        let run = parsed_id(&id)?;
        let Some(mut detail) = run_detail(db, &run)? else {
            continue;
        };
        detail.outcomes = outcomes(db, &run, query)?;
        if !(narrowed && detail.outcomes.is_empty()) {
            runs.push(detail);
        }
    }
    if runs.iter().all(|r| r.outcomes.is_empty())
        && let Some(unknown) = unknown_glob(db, query)?
    {
        return Err(unknown.into());
    }
    Ok(runs)
}

fn parsed_id(id: &str) -> Result<RunId, IndexError> {
    Ok(parsed(0, RunId::parse(id))?)
}

/// The run `run` as the index holds it, its outcomes not yet read; `None`
/// when no counted run has that id.
fn run_detail(db: &Connection, run: &RunId) -> Result<Option<RunDetail>, IndexError> {
    Ok(db
        .query_row(
            "SELECT run, kind, tree, variant, open, opened, closed, machine, conditions,
                    ended, reason, detail
             FROM counted_runs WHERE run = ?1",
            [run.as_str()],
            |row| {
                let open: String = row.get(4)?;
                Ok(RunDetail {
                    run: parsed(0, RunId::parse(&row.get::<_, String>(0)?))?,
                    kind: parsed(1, Kind::parse(&row.get::<_, String>(1)?))?,
                    tree: parsed(2, TreeId::parse(&row.get::<_, String>(2)?))?,
                    machine: machine_at(row, 7)?,
                    variant: parsed(3, Sha256::parse(&row.get::<_, String>(3)?))?,
                    params: parsed(4, params_of(&open))?,
                    opened: parsed(5, Timestamp::parse(&row.get::<_, String>(5)?))?,
                    closed: time_at(row, 6)?,
                    end: end_at(row, 9)?,
                    open: parsed(4, serde_json::from_str(&open))?,
                    conditions: row
                        .get::<_, Option<String>>(8)?
                        .map(|text| parsed(8, serde_json::from_str(&text)))
                        .transpose()?,
                    outcomes: Vec::new(),
                })
            },
        )
        .optional()?)
}

/// Why there is no run `run`: recorded but unreadable, or not recorded.
fn missing_run(db: &Connection, run: &RunId) -> Result<NotRecorded, IndexError> {
    let problem: Option<String> = db
        .query_row(
            "SELECT problem FROM all_violations WHERE run = ?1 ORDER BY problem LIMIT 1",
            [run.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match problem {
        Some(problem) => NotRecorded::Unreadable {
            run: run.clone(),
            problem,
        },
        None => NotRecorded::Run(run.clone()),
    })
}

/// The run's outcomes the key and metric globs keep: a measured outcome
/// with its measures the metric glob matches, any other outcome when its
/// name (`verdict`, `refused`, `skipped`) does.
fn outcomes(
    db: &Connection,
    run: &RunId,
    query: &ShowQuery,
) -> Result<Vec<KeyOutcome>, IndexError> {
    let mut measures = measures(db, run, query)?;
    let outcomes = db
        .prepare(
            "SELECT key, outcome, reason, detail, status, location
             FROM results WHERE run = ?1 AND (?2 IS NULL OR key GLOB ?2) ORDER BY key",
        )?
        .query_map(params![run, query.key], |row| {
            let key: String = row.get(0)?;
            let measures = measures.remove(&key).unwrap_or_default();
            Ok(KeyOutcome {
                key: parsed(0, Key::parse(&key))?,
                outcome: shown_outcome(row, measures)?,
                location: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let Some(glob) = &query.metric else {
        return Ok(outcomes);
    };
    let matches = |name: &str| -> rusqlite::Result<bool> {
        db.query_row("SELECT ?1 GLOB ?2", params![name, glob], |r| r.get(0))
    };
    let mut kept = Vec::new();
    for outcome in outcomes {
        let named = !matches!(outcome.outcome, ShownOutcome::Measured(_))
            && matches(outcome.outcome.name())?;
        if !outcome.outcome.measures().is_empty() || named {
            kept.push(outcome);
        }
    }
    Ok(kept)
}

/// A `results` row's outcome (columns 1 to 4: `outcome`, `reason`,
/// `detail`, `status`), holding `measures`.
fn shown_outcome(
    row: &rusqlite::Row<'_>,
    measures: Vec<DeclaredMeasure>,
) -> rusqlite::Result<ShownOutcome> {
    let reason = || -> rusqlite::Result<Reason> {
        Ok(Reason {
            reason: row.get(2)?,
            detail: row.get(3)?,
        })
    };
    let name: String = row.get(1)?;
    Ok(match name.as_str() {
        "measured" => ShownOutcome::Measured(measures),
        "verdict" => ShownOutcome::Verdict {
            status: tag_at::<VerdictStatus>(row, 4)?,
            measures,
        },
        "refused" => ShownOutcome::Refused(reason()?),
        "skipped" => ShownOutcome::Skipped(reason()?),
        other => return Err(unreadable(1, format!("no outcome is {other:?}"))),
    })
}

/// The run's numbers the key and metric globs keep, by key, each key's in
/// metric order.
fn measures(
    db: &Connection,
    run: &RunId,
    query: &ShowQuery,
) -> Result<BTreeMap<String, Vec<DeclaredMeasure>>, IndexError> {
    let mut by_key: BTreeMap<String, Vec<DeclaredMeasure>> = BTreeMap::new();
    let rows = db
        .prepare(
            "SELECT key, metric, value, unit, better, n, mad, origin FROM all_measures
             WHERE run = ?1 AND (?2 IS NULL OR key GLOB ?2) AND (?3 IS NULL OR metric GLOB ?3)
             ORDER BY key, metric",
        )?
        .query_map(params![run, query.key, query.metric], |row| {
            let better: Option<String> = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                DeclaredMeasure {
                    metric: parsed(1, MetricName::parse(&row.get::<_, String>(1)?))?,
                    value: row.get(2)?,
                    unit: row.get(3)?,
                    better: better
                        .map(|text| {
                            parsed(4, serde_json::from_value::<Direction>(Value::String(text)))
                        })
                        .transpose()?,
                    n: row.get(5)?,
                    mad: row.get(6)?,
                    origin: tag_at(row, 7)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (key, measure) in rows {
        by_key.entry(key).or_default().push(measure);
    }
    Ok(by_key)
}

/// A key or metric glob that matches nothing recorded anywhere, with the
/// nearest recorded names: a typo, never an empty answer.
fn unknown_glob(db: &Connection, query: &ShowQuery) -> Result<Option<NotRecorded>, IndexError> {
    let nowhere = |sql: &str, glob: &str| -> rusqlite::Result<bool> {
        db.query_row(sql, [glob], |r| r.get::<_, bool>(0))
            .map(|any| !any)
    };
    if let Some(glob) = &query.key
        && nowhere(
            "SELECT EXISTS (SELECT 1 FROM results WHERE key GLOB ?1)",
            glob,
        )?
    {
        let names = column_values_with(db, "SELECT DISTINCT key FROM results", [])?;
        return Ok(Some(NotRecorded::Key {
            key: glob.clone(),
            nearest: nearest(glob, &names),
        }));
    }
    if let Some(glob) = &query.metric
        && nowhere(
            "SELECT EXISTS (SELECT 1 FROM all_measures WHERE metric GLOB ?1)
             OR EXISTS (SELECT 1 FROM results WHERE outcome != 'measured' AND outcome GLOB ?1)",
            glob,
        )?
    {
        let names = column_values_with(db, "SELECT DISTINCT metric FROM all_measures", [])?;
        return Ok(Some(NotRecorded::MetricGlob {
            glob: glob.clone(),
            nearest: nearest(glob, &names),
        }));
    }
    Ok(None)
}

/// One run: its parameters first, one row each (`metric` is `param.NAME`,
/// `key` empty), then its close conditions, one row per value
/// (`condition.PATH`, a JSON `null` an empty cell); then its outcome rows,
/// as for a revision.
impl TidyRows for RunDetail {
    fn header(&self) -> Vec<String> {
        columns(&SHOWN)
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        let mut rows: Vec<Vec<Option<String>>> = (self.params.iter())
            .map(|param| {
                let metric = cell(format!("param.{}", param.name));
                shown_row(self, None, metric, cell(&param.value), None)
            })
            .collect();
        if let Some(conditions) = &self.conditions {
            rows.extend(
                condition_leaves(conditions)
                    .into_iter()
                    .map(|(path, value)| {
                        let metric = if path.is_empty() {
                            "condition".to_owned()
                        } else {
                            format!("condition.{path}")
                        };
                        shown_row(self, None, Some(metric), value, None)
                    }),
            );
        }
        rows.extend(outcome_rows(self));
        rows
    }
}

/// Every run on a revision, run by run: one row per recorded number, and one
/// per key that recorded no number, its `metric` the outcome (`verdict`,
/// `refused`, `skipped`) and its `value` the verdict's status or the reason.
impl TidyRows for Vec<RunDetail> {
    fn header(&self) -> Vec<String> {
        columns(&SHOWN)
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.iter().flat_map(outcome_rows).collect()
    }
}

/// The columns `show` prints.
const SHOWN: [&str; 7] = ["kind", "machine", "key", "metric", "value", "unit", "run"];

/// One row of `show` about `run`.
fn shown_row(
    run: &RunDetail,
    key: Option<String>,
    metric: Option<String>,
    value: Option<String>,
    unit: Option<String>,
) -> Vec<Option<String>> {
    vec![
        cell(&run.kind),
        run.machine.as_ref().map(ToString::to_string),
        key,
        metric,
        value,
        unit,
        cell(&run.run),
    ]
}

/// A run's outcomes as rows: one per recorded number, and one per key that
/// recorded no number, its `metric` the outcome (`verdict`, `refused`,
/// `skipped`) and its `value` the verdict's status or the reason. A verdict's
/// derived durations follow its row.
fn outcome_rows(run: &RunDetail) -> Vec<Vec<Option<String>>> {
    let mut rows = Vec::new();
    for outcome in &run.outcomes {
        let key = cell(&outcome.key);
        let said = match &outcome.outcome {
            ShownOutcome::Measured(_) => None,
            ShownOutcome::Verdict { status, .. } => Some(status.as_str()),
            ShownOutcome::Refused(reason) | ShownOutcome::Skipped(reason) => {
                Some(reason.reason.as_str())
            }
        };
        if let Some(said) = said {
            let name = cell(outcome.outcome.name());
            rows.push(shown_row(run, key.clone(), name, cell(said), None));
        }
        for m in outcome.outcome.measures() {
            rows.push(shown_row(
                run,
                key.clone(),
                cell(&m.metric),
                cell(m.value),
                cell(&m.unit),
            ));
        }
    }
    rows
}
