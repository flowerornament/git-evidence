//! A run that names no machine, end to end: every verb that shows a machine
//! prints an empty CSV field and a blank cell in its aligned columns, and the
//! SQL views hold NULL; never a placeholder such as `""` or `unknown`.

use crate::common::{Fixture, Shape, fixture, p99, perf, record_run, run_ids};
use git_evidence::End;
use std::process::Command;

/// Two landings, each with one perf run whose open names no machine.
fn store() -> Fixture {
    let f = fixture(2);
    let nameless = Shape {
        machine: "",
        ..Shape::default()
    };
    for (landing, value) in [(0, 100.0), (1, 104.0)] {
        record_run(
            &f,
            &perf(),
            landing,
            &nameless,
            &[("parse", p99(value))],
            Some(End::Complete),
        );
    }
    f
}

fn stdout(f: &Fixture, csv: bool, args: &[&str]) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_git-evidence"));
    command
        .current_dir(f.dir.path())
        .args(["--trunk", "refs/heads/master"]);
    if csv {
        command.arg("--csv");
    }
    let output = command.args(args).output().expect("CLI");
    assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).expect("UTF-8")
}

/// One CSV line's fields, quotes undone.
fn fields(line: &str) -> Vec<String> {
    let (mut fields, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    fields.push(field);
    fields
}

/// Every row of `args`'s CSV has an empty `machine` field.
fn csv_machine_is_empty(f: &Fixture, args: &[&str]) {
    let text = stdout(f, true, args);
    let mut lines = text.lines();
    let header = fields(lines.next().expect("a header"));
    let column = header
        .iter()
        .position(|name| name == "machine")
        .unwrap_or_else(|| panic!("{args:?}: no machine column in {header:?}"));
    let rows: Vec<Vec<String>> = lines.map(fields).collect();
    assert!(!rows.is_empty(), "{args:?}: rows to check");
    for row in rows {
        assert_eq!(row[column], "", "{args:?}: {row:?}");
    }
}

/// Every row of `args`'s aligned columns is blank under `machine`.
fn text_machine_is_blank(f: &Fixture, args: &[&str]) {
    let text = stdout(f, false, args);
    let mut lines = text.lines();
    let header = lines.next().expect("a header");
    let start = header
        .find("machine")
        .unwrap_or_else(|| panic!("{args:?}: no machine column in {header:?}"));
    let cell = start..start + "machine".len();
    let rows: Vec<&str> = lines.collect();
    assert!(!rows.is_empty(), "{args:?}: rows to check");
    for row in rows {
        let shown = row.get(cell.clone()).unwrap_or_default();
        assert!(shown.trim().is_empty(), "{args:?}: {row:?}");
    }
}

#[test]
fn a_run_naming_no_machine_shows_an_empty_machine_in_every_verb() {
    let f = store();
    let run = run_ids(&f, &perf()).remove(0);
    let (before, after) = (f.landings[0].as_str(), f.landings[1].as_str());
    let verbs: [&[&str]; 5] = [
        &["show", after],
        &["show", run.as_str()],
        &["log", after],
        &["trend", "block.p99_ns", "parse"],
        &["diff", before, after],
    ];
    for args in verbs {
        csv_machine_is_empty(&f, args);
        text_machine_is_blank(&f, args);
    }
    let null = stdout(
        &f,
        true,
        &[
            "sql",
            "SELECT count(*) AS runs, count(machine) AS named FROM runs",
        ],
    );
    assert_eq!(null, "runs,named\n2,0\n", "machine is NULL in the views");
}
