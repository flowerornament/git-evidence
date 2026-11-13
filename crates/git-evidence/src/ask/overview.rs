//! `git evidence` with no verb: the newest landings and how many runs of each
//! kind were recorded on each, refused ones counted apart.

use super::landings::{Landing, LandingNumber, at_positions};
use super::selector::{Selector, json, landings};
use crate::index::IndexError;
use crate::rows::{TidyRows, cell, columns};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// The newest landings, oldest first (as `log` and `trend` list them), each
/// with its run counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Overview {
    pub landings: Vec<LandingSummary>,
}

/// One landing and, per kind, the runs recorded on its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct LandingSummary {
    #[serde(flatten)]
    pub landing: Landing,
    /// Kind → runs; a kind with no run on this tree is absent.
    pub runs: BTreeMap<String, RunCount>,
}

/// How many runs of one kind a tree has, and how many of them ended refused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RunCount {
    pub runs: i64,
    pub refused: i64,
}

impl Overview {
    /// Every kind with a run on any landing shown, in name order.
    #[must_use]
    pub fn kinds(&self) -> Vec<&str> {
        let mut kinds: Vec<&str> = (self.landings.iter())
            .flat_map(|l| l.runs.keys().map(String::as_str))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }

    /// The kinds with a refused run on any landing shown, in name order.
    #[must_use]
    pub fn refusing_kinds(&self) -> Vec<&str> {
        let mut kinds: Vec<&str> = (self.landings.iter())
            .flat_map(|l| l.runs.iter())
            .filter(|(_, count)| count.refused > 0)
            .map(|(kind, _)| kind.as_str())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds
    }
}

/// The newest `last` landings with their run counts.
///
/// # Errors
/// An index failure.
pub(crate) fn overview(db: &Connection, last: NonZeroU32) -> Result<Overview, crate::Error> {
    let positions = json(&landings(db, &Selector::newest(last))?);
    let mut counts: BTreeMap<LandingNumber, BTreeMap<String, RunCount>> = BTreeMap::new();
    let rows = db
        .prepare(
            "SELECT l.landing, r.kind, COUNT(*), SUM(r.ended IS 'refused')
             FROM trunk l JOIN counted_runs r ON r.tree = l.tree
             WHERE l.landing IN (SELECT value FROM json_each(?1))
             GROUP BY 1, 2",
        )?
        .query_map([&positions], |row| {
            Ok((
                row.get::<_, LandingNumber>(0)?,
                row.get::<_, String>(1)?,
                RunCount {
                    runs: row.get(2)?,
                    refused: row.get(3)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(IndexError::from)?;
    for (landing, kind, count) in rows {
        counts.entry(landing).or_default().insert(kind, count);
    }
    let landings = at_positions(db, &positions)?
        .into_iter()
        .map(|landing| LandingSummary {
            runs: counts.remove(&landing.number).unwrap_or_default(),
            landing,
        })
        .collect();
    Ok(Overview { landings })
}

/// One row per landing, oldest first: its run count of each kind, then of
/// refused runs for each kind that has any, then its subject.
impl TidyRows for Overview {
    fn header(&self) -> Vec<String> {
        let mut header = columns(&["landing", "commit", "date"]);
        header.extend(self.kinds().into_iter().map(str::to_owned));
        header.extend(self.refusing_kinds().iter().map(|k| format!("{k}_refused")));
        header.push("subject".to_owned());
        header
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        let (kinds, refusing) = (self.kinds(), self.refusing_kinds());
        self.landings
            .iter()
            .map(|summary| {
                let l = &summary.landing;
                let count = |kind: &str| summary.runs.get(kind).copied().unwrap_or_default();
                let mut row = vec![cell(l.number), cell(&l.commit), cell(&l.date)];
                row.extend(kinds.iter().map(|kind| cell(count(kind).runs)));
                row.extend(refusing.iter().map(|kind| cell(count(kind).refused)));
                row.push(cell(&l.subject));
                row
            })
            .collect()
    }
}
