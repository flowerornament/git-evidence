//! A note's runs as table rows: a changed note is folded and its rows
//! replaced whole (runs, parameters, close conditions, outcomes, measures and
//! violations), and
//! a run id seen on more than one tree is excluded from every one of them.

use super::schema::{RUN_ROWS, execute_cached};
use crate::format::event::{
    DERIVED_PREFIX, Direction, End, Metric, Outcome, Timestamp, Try, variant_of,
};
use crate::format::fold::{RunState, fold};
use crate::format::id::{Key, Machine, MetricName, RunId, TreeId};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn delete_note_rows(db: &Connection, kind: &str, tree: &TreeId) -> rusqlite::Result<()> {
    for table in RUN_ROWS {
        execute_cached(
            db,
            &format!(
                "DELETE FROM {table} WHERE run IN (SELECT run FROM all_runs WHERE kind = ?1 AND tree = ?2)"
            ),
            params![kind, tree.as_str()],
        )?;
    }
    execute_cached(
        db,
        "DELETE FROM all_runs WHERE kind = ?1 AND tree = ?2",
        params![kind, tree.as_str()],
    )?;
    execute_cached(
        db,
        "DELETE FROM violations WHERE kind = ?1 AND tree = ?2",
        params![kind, tree.as_str()],
    )?;
    execute_cached(
        db,
        "DELETE FROM sightings WHERE kind = ?1 AND tree = ?2",
        params![kind, tree.as_str()],
    )?;
    Ok(())
}

/// What an outcome says, as a digest: equal outcomes hash equal whatever order
/// their measures were listed in, and a measure's unit and direction are part of
/// what it says.
fn outcome_digest(
    outcome: &Outcome,
    metrics: &BTreeMap<MetricName, Metric>,
) -> crate::format::id::Sha256 {
    let said = match outcome {
        Outcome::Measured(_) => {
            let mut measures = declared_measures(outcome, metrics);
            measures.sort_by(|a, b| a.metric.as_str().cmp(b.metric.as_str()));
            serde_json::json!({ "measured": measures })
        }
        other => serde_json::to_value(other).expect("plain data serializes"),
    };
    crate::format::id::Sha256::of(&crate::format::event::canonical_json(&said))
}

/// Removes a run admitted from another note.
fn delete_run(db: &Connection, run: &str) -> rusqlite::Result<()> {
    for table in RUN_ROWS.iter().chain(&["all_runs"]) {
        execute_cached(db, &format!("DELETE FROM {table} WHERE run = ?1"), [run])?;
    }
    Ok(())
}

/// Records `ids` as sighted in `kind`'s note on `tree`, and excludes every
/// run id now seen on more than one tree: its rows are deleted, and each
/// copy carries the same violation, so the result is the same whichever
/// note was folded first. Returns the ids excluded.
fn exclude_copies<'a>(
    db: &Connection,
    kind: &str,
    tree: &TreeId,
    ids: BTreeSet<&'a str>,
) -> rusqlite::Result<BTreeSet<&'a str>> {
    for id in &ids {
        execute_cached(
            db,
            "INSERT OR IGNORE INTO sightings (run, kind, tree) VALUES (?1, ?2, ?3)",
            params![id, kind, tree.as_str()],
        )?;
    }
    let mut excluded = BTreeSet::new();
    for id in ids {
        let seen: Vec<(String, String)> = db
            .prepare_cached("SELECT kind, tree FROM sightings WHERE run = ?1 ORDER BY kind, tree")?
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        if seen.len() < 2 {
            continue;
        }
        excluded.insert(id);
        delete_run(db, id)?;
        let problem = format!(
            "recorded in {} places ({}); every copy is excluded",
            seen.len(),
            seen.iter()
                .map(|(kind, tree)| format!("{kind} {tree}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        execute_cached(
            db,
            "DELETE FROM violations WHERE run = ?1 AND cause = 'copies'",
            [id],
        )?;
        for (other_kind, other_tree) in &seen {
            execute_cached(
                db,
                "INSERT INTO violations (kind, tree, run, problem, cause)
                 VALUES (?1, ?2, ?3, ?4, 'copies')",
                params![other_kind, other_tree, id, problem],
            )?;
        }
    }
    Ok(excluded)
}

pub(super) fn insert_note(
    db: &Connection,
    kind: &str,
    tree: &TreeId,
    note: &[u8],
) -> rusqlite::Result<()> {
    let folded = fold(note);
    let violations: Vec<(Option<String>, String)> = folded
        .violations
        .iter()
        .map(|v| {
            (
                v.run.as_ref().map(ToString::to_string),
                v.problem.to_string(),
            )
        })
        .collect();
    // Every run id in the note is a sighting, whether or not it folds, so a
    // copy that breaks elsewhere still excludes this one.
    let ids: BTreeSet<&str> = folded
        .runs
        .iter()
        .map(|run| run.id.as_str())
        .chain(
            folded
                .violations
                .iter()
                .filter_map(|v| v.run.as_ref().map(RunId::as_str)),
        )
        .collect();
    let excluded = exclude_copies(db, kind, tree, ids)?;
    for run in &folded.runs {
        if !excluded.contains(run.id.as_str()) {
            insert_run(db, kind, tree, run)?;
        }
    }
    for (run, problem) in violations {
        execute_cached(
            db,
            "INSERT INTO violations (kind, tree, run, problem, cause)
             VALUES (?1, ?2, ?3, ?4, 'fold')",
            params![kind, tree.as_str(), run, problem],
        )?;
    }
    Ok(())
}

/// A parameter's text: strings as themselves, other JSON as JSON.
fn param_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

fn insert_run(db: &Connection, kind: &str, tree: &TreeId, run: &RunState) -> rusqlite::Result<()> {
    // The open as completed by `declare`: its build and keys are part of what
    // the run is, so they enter the variant and what `show` prints.
    let open = crate::format::event::Open {
        build: run.build.clone(),
        keys: run.keys.clone(),
        ..run.open.clone()
    };
    let variant = variant_of(kind, &open);
    // fold() reads no run whose machine does not parse: a run here names
    // none (NULL) or a machine.
    let machine = run.machine();
    debug_assert!(
        machine.is_ok(),
        "fold() read a run whose machine does not parse"
    );
    let machine = machine.ok().flatten();
    let (ended, reason) = match &run.end {
        None => (None, None),
        Some(End::Complete) => (Some("complete"), None),
        Some(End::Refused(reason)) => (Some("refused"), Some(reason)),
    };
    // Written as the JSON every reader parses back; a failure is the write's.
    let open_json = serde_json::to_string(&open)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    execute_cached(db,
        "INSERT INTO all_runs (run, kind, tree, commit_id, producer, variant, machine, opened, closed,
           ended, reason, detail, open, conditions)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            run.id.as_str(),
            kind,
            tree.as_str(),
            run.open.commit.as_str(),
            run.open.producer.name.as_str(),
            variant.as_str(),
            machine.as_ref().map(Machine::as_str),
            run.opened.as_str(),
            run.closed.as_ref().map(Timestamp::as_str),
            ended,
            reason.map(|r| r.reason.as_str()),
            reason.and_then(|r| r.detail.as_deref()),
            open_json,
            run.conditions.as_ref().map(serde_json::Value::to_string),
        ],
    )?;
    for key in run.keys.iter().flatten() {
        execute_cached(
            db,
            "INSERT OR IGNORE INTO run_keys (run, key) VALUES (?1, ?2)",
            params![run.id.as_str(), key.as_str()],
        )?;
    }
    insert_params(db, run)?;
    for (path, value) in run
        .conditions
        .iter()
        .flat_map(crate::rows::condition_leaves)
    {
        execute_cached(
            db,
            "INSERT INTO run_conditions (run, path, value) VALUES (?1, ?2, ?3)",
            params![run.id.as_str(), path, value],
        )?;
    }
    for key in run.keys.iter().flatten() {
        if let Some(outcome) = run.outcome(key) {
            insert_outcome(db, run, key, &outcome)?;
        }
    }
    Ok(())
}

/// A run's parameters: its environment's, and its suite's (as text).
fn insert_params(db: &Connection, run: &RunState) -> rusqlite::Result<()> {
    let suite = run
        .open
        .suite
        .params
        .iter()
        .map(|(name, value)| (name.clone(), param_text(value)));
    let environment = run
        .open
        .environment
        .params
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()));
    for (name, value) in environment.chain(suite) {
        execute_cached(
            db,
            "INSERT OR IGNORE INTO run_params (run, name, value) VALUES (?1, ?2, ?3)",
            params![run.id.as_str(), name, value],
        )?;
    }
    Ok(())
}

/// An outcome's columns in `results`: its kind, and what that kind carries.
struct OutcomeRow<'a> {
    kind: &'static str,
    reason: Option<&'a str>,
    detail: Option<&'a str>,
    status: Option<&'a str>,
    tries: Option<i64>,
}

impl<'a> OutcomeRow<'a> {
    fn of(outcome: &'a Outcome) -> Self {
        let row = |kind| Self {
            kind,
            reason: None,
            detail: None,
            status: None,
            tries: None,
        };
        match outcome {
            Outcome::Measured(_) => row("measured"),
            Outcome::Verdict(verdict) => Self {
                status: Some(verdict.status.as_str()),
                tries: i64::try_from(verdict.tries.len()).ok(),
                ..row("verdict")
            },
            Outcome::Refused(reason) | Outcome::Skipped(reason) => Self {
                reason: Some(reason.reason.as_str()),
                detail: reason.detail.as_deref(),
                ..row(if matches!(outcome, Outcome::Refused(_)) {
                    "refused"
                } else {
                    "skipped"
                })
            },
        }
    }
}

/// One key's outcome in `run`: its result row, and the measures it gives.
fn insert_outcome(
    db: &Connection,
    run: &RunState,
    key: &Key,
    outcome: &Outcome,
) -> rusqlite::Result<()> {
    let location = run.results.get(key).and_then(|a| a.location.clone());
    let row = OutcomeRow::of(outcome);
    execute_cached(
        db,
        "INSERT INTO results (run, key, outcome, reason, detail, status, tries, location, digest)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            run.id.as_str(),
            key.as_str(),
            row.kind,
            row.reason,
            row.detail,
            row.status,
            row.tries,
            location,
            outcome_digest(outcome, &run.metrics).as_str(),
        ],
    )?;
    for measure in declared_measures(outcome, &run.metrics) {
        execute_cached(db,
                "INSERT OR REPLACE INTO all_measures (run, key, metric, value, unit, better, n, mad, origin)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    run.id.as_str(),
                    key.as_str(),
                    measure.metric.as_str(),
                    measure.value,
                    measure.unit,
                    measure.better.map(direction_text),
                    measure.n,
                    measure.mad,
                    measure.origin.as_str(),
                ],
            )?;
    }
    Ok(())
}

/// A number as the index holds it: a measure beside its run's declaration of
/// the metric (its unit and which way is better), and whether a producer
/// recorded it or the index derived it from a verdict's tries. Serialized,
/// part of an outcome's digest.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[non_exhaustive]
pub struct DeclaredMeasure {
    pub metric: MetricName,
    pub value: f64,
    /// The declared UCUM unit.
    pub unit: String,
    pub better: Option<Direction>,
    pub n: Option<u32>,
    pub mad: Option<f64>,
    pub origin: MeasureOrigin,
}

/// Where a number came from: a producer's run, or the index, which derives a
/// verdict's times (`verdict.<status>.duration_ms`, `….cpu_ms`) from its tries
/// and never stores them in notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MeasureOrigin {
    Recorded,
    Derived,
}

impl MeasureOrigin {
    /// The origin as the index and the public `measures` view spell it.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Derived => "derived",
        }
    }
}

/// A direction's spelling in the index (and in rows printed from it).
pub(crate) fn direction_text(better: Direction) -> &'static str {
    match better {
        Direction::Lower => "lower",
        Direction::Higher => "higher",
    }
}

/// A stored direction's spelling read back: the inverse of
/// [`direction_text`], the one place both spellings live.
pub(crate) fn direction_from_text(text: &str) -> Result<Direction, UnknownDirection> {
    [Direction::Lower, Direction::Higher]
        .into_iter()
        .find(|direction| direction_text(*direction) == text)
        .ok_or_else(|| UnknownDirection(text.to_owned()))
}

/// A stored direction spelled neither `lower` nor `higher`.
#[derive(Debug, thiserror::Error)]
#[error("unknown direction {0:?} in the index")]
pub(crate) struct UnknownDirection(String);

/// The measures an outcome gives: each recorded measure with its run's
/// declaration (the fold admitted only declared metrics), and for a verdict
/// `verdict.<status>.duration_ms` and `verdict.<status>.cpu_ms`, each the sum
/// of its tries' wall or CPU times. A time is derived only when every try has
/// one and the sum is finite: a missing time is never
/// read as zero.
fn declared_measures(
    outcome: &Outcome,
    metrics: &BTreeMap<MetricName, Metric>,
) -> Vec<DeclaredMeasure> {
    match outcome {
        Outcome::Measured(measures) => measures
            .iter()
            .filter_map(|measure| {
                let declared = metrics.get(&measure.metric)?;
                Some(DeclaredMeasure {
                    metric: measure.metric.clone(),
                    value: measure.value,
                    unit: declared.unit.clone(),
                    better: declared.better,
                    n: measure.n,
                    mad: measure.mad,
                    origin: MeasureOrigin::Recorded,
                })
            })
            .collect(),
        Outcome::Verdict(verdict) => {
            let status = verdict.status.as_str();
            Try::TIMES
                .iter()
                .filter_map(|(name, time)| {
                    let times: Option<Vec<f64>> = verdict.tries.iter().map(time).collect();
                    let total = times
                        .filter(|t| !t.is_empty())
                        .map(|t| t.iter().sum::<f64>())
                        .filter(|total| total.is_finite())?;
                    Some(DeclaredMeasure {
                        metric: MetricName::parse(&format!("{DERIVED_PREFIX}{status}.{name}"))
                            .ok()?,
                        value: total,
                        unit: "ms".to_owned(),
                        better: Some(Direction::Lower),
                        n: u32::try_from(verdict.tries.len()).ok(),
                        mad: None,
                        origin: MeasureOrigin::Derived,
                    })
                })
                .collect()
        }
        Outcome::Refused(_) | Outcome::Skipped(_) => Vec::new(),
    }
}
