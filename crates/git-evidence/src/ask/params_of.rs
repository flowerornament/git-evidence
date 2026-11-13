//! A run's parameters, readably: the producer, harness and parameters its
//! open names. Two runs with different parameters are different variants,
//! and their numbers are never matched with each other.

use super::show::Param;

/// The parameters a run's open names, as the index stores the open: the
/// producer, the harness, then the environment's, the suite's and the
/// build's parameters, each as a name and its text. The machine is not
/// among them; it is a column of its own wherever runs are listed. An open
/// the index cannot parse is an error, never an empty list.
pub(super) fn params_of(open: &str) -> serde_json::Result<Vec<Param>> {
    let open: serde_json::Value = serde_json::from_str(open)?;
    let mut params = Vec::new();
    for (name, pointer) in [
        ("producer", "/producer/name"),
        ("harness", "/suite/harness"),
    ] {
        if let Some(text) = open.pointer(pointer).and_then(|v| v.as_str())
            && !text.is_empty()
        {
            params.push(Param {
                name: name.to_owned(),
                value: text.to_owned(),
            });
        }
    }
    for section in ["/environment/params", "/suite/params", "/build/profile"] {
        if let Some(fields) = open.pointer(section).and_then(|v| v.as_object()) {
            for (name, value) in fields {
                let value = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                params.push(Param {
                    name: name.clone(),
                    value,
                });
            }
        }
    }
    Ok(params)
}
