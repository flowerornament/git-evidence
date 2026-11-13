//! "Did you mean": a key or metric nothing in the store has is
//! [`NotRecorded`], with the nearest recorded names by edit distance, so a
//! typo never reads as an empty answer.

use super::NotRecorded;
use crate::format::event::{DERIVED_PREFIX, Try};
use crate::index::IndexError;
use crate::index::{column_values, column_values_with};
use rusqlite::{Connection, params};

/// [`NotRecorded::Key`] or [`NotRecorded::Metric`], with the nearest names,
/// when nothing in the store has the key, or the key with the metric.
pub(super) fn unknown_name(
    db: &Connection,
    key: &str,
    metric: &str,
) -> Result<Option<NotRecorded>, IndexError> {
    let has_key: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM run_keys k JOIN counted_runs r ON r.run = k.run WHERE k.key = ?1)",
        [key],
        |r| r.get(0),
    )?;
    if !has_key {
        let names = column_values(
            db,
            "SELECT DISTINCT k.key FROM run_keys k JOIN counted_runs r ON r.run = k.run",
        )?;
        return Ok(Some(NotRecorded::Key {
            key: key.to_owned(),
            nearest: nearest(key, &names),
        }));
    }
    let has_metric: bool = db.query_row(
        "SELECT EXISTS (SELECT 1 FROM all_measures m JOIN counted_runs r ON r.run = m.run
                        WHERE m.key = ?1 AND m.metric = ?2)",
        params![key, metric],
        |r| r.get(0),
    )?;
    // A derived metric (a verdict's duration) is the index's own: for a key
    // with verdicts it is a known name even where no try yielded it.
    let derived = crate::format::event::VerdictStatus::ALL
        .iter()
        .flat_map(|status| Try::TIMES.iter().map(move |(name, _)| (status, name)))
        .any(|(status, name)| metric == format!("{DERIVED_PREFIX}{}.{name}", status.as_str()))
        && db.query_row(
            "SELECT EXISTS (SELECT 1 FROM results o JOIN counted_runs r ON r.run = o.run
                            WHERE o.key = ?1 AND o.outcome = 'verdict')",
            params![key],
            |row| row.get::<_, bool>(0),
        )?;
    if !has_metric && !derived {
        let names = column_values_with(
            db,
            "SELECT DISTINCT m.metric FROM all_measures m JOIN counted_runs r ON r.run = m.run WHERE m.key = ?1",
            [key],
        )?;
        return Ok(Some(NotRecorded::Metric {
            key: key.to_owned(),
            metric: metric.to_owned(),
            nearest: nearest(metric, &names),
        }));
    }
    Ok(None)
}

/// The three names closest to `asked` by edit distance, nearest first.
pub(super) fn nearest(asked: &str, names: &[String]) -> Vec<String> {
    let mut ranked: Vec<(usize, &String)> = names
        .iter()
        .map(|name| (distance(asked, name), name))
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .take(3)
        .map(|(_, name)| name.clone())
        .collect()
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_names_rank_by_edit_distance() {
        let names = ["parse_large_64", "parse_large_16", "parse_small"].map(String::from);
        assert_eq!(nearest("parse_large_d64", &names)[0], "parse_large_64");
        assert!(nearest("x", &[]).is_empty());
    }
}
