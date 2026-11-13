//! Every answer as rows: one fact per row, under a header. The command line
//! prints them as aligned columns, or with `--csv` as CSV; either way the
//! columns are the same, and a value the answer does not hold is an empty
//! cell, never a zero or a placeholder.
//!
//! ```text
//! answer ─▶ header() + rows() ─┬─ aligned columns   (a terminal)
//!                              └─ --csv             (a script)
//! ```

use serde_json::Value;

/// An answer as rows under a header.
pub trait TidyRows {
    /// The column names, in order. Every row has one cell per column.
    fn header(&self) -> Vec<String>;

    /// The rows, in the answer's order. `None` is a value the answer does not
    /// hold.
    fn rows(&self) -> Vec<Vec<Option<String>>>;
}

/// A fixed header.
pub(crate) fn columns(names: &[&str]) -> Vec<String> {
    names.iter().map(|&name| name.to_owned()).collect()
}

/// A cell holding a value.
#[expect(
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "a row is cells that may be empty; callers pass numbers and references alike"
)]
pub(crate) fn cell(value: impl ToString) -> Option<String> {
    Some(value.to_string())
}

/// A JSON scalar as a cell: text as itself, a number as every other number
/// cell spells it (`88634`, not `88634.0`), `null` as no value.
pub(crate) fn json_cell(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(
            number
                .as_f64()
                .map_or_else(|| number.to_string(), |n| n.to_string()),
        ),
        other => Some(other.to_string()),
    }
}

/// Each scalar inside a run's close `conditions`, with its dotted path (an
/// object field by its name, an array element by its index) and its value
/// as a cell (text as itself, a number as every number cell spells it, `null`
/// as no value); an empty object or array is one leaf, as written,
/// and the conditions themselves, when they are a scalar or empty, are the
/// one leaf at the empty path. `show RUN` prints these rows and the
/// `conditions` view holds them; a producer that queries its own conditions
/// spells their paths with it.
#[must_use]
pub fn condition_leaves(conditions: &Value) -> Vec<(String, Option<String>)> {
    fn walk(path: &str, value: &Value, out: &mut Vec<(String, Option<String>)>) {
        let children: Vec<(String, &Value)> = match value {
            Value::Object(fields) => fields.iter().map(|(k, v)| (k.clone(), v)).collect(),
            Value::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect(),
            scalar => {
                out.push((path.to_owned(), json_cell(scalar)));
                return;
            }
        };
        if children.is_empty() {
            out.push((path.to_owned(), Some(value.to_string())));
        }
        for (name, child) in children {
            let path = if path.is_empty() {
                name
            } else {
                format!("{path}.{name}")
            };
            walk(&path, child, out);
        }
    }
    let mut leaves = Vec::new();
    walk("", conditions, &mut leaves);
    leaves
}
