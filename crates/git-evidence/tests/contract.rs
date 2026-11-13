//! The contract git-evidence keeps with its users: the names its crate root
//! exports and the columns of its public SQL views, each pinned in a
//! checked-in file that changes only with a deliberate change of contract.
//!
//! The crate root is the whole Rust contract.
//! The surface test reads `src/lib.rs` and compares every name it exports with the
//! checked-in list `tests/public-surface.txt`: an export added, removed or
//! renamed without updating the list fails here, and so does any public item
//! other than a `pub use` re-export or the feature-gated `testing` module.
//!
//! A text check of `lib.rs` suffices because every other module is private:
//! nothing outside a `pub use` there can be named from outside the crate.
//! (`cargo public-api` would read rustdoc JSON, which needs a nightly
//! toolchain the pinned shell does not carry.)
//!
//! The public views are a contract versioned by `PUBLIC_SCHEMA`: their
//! names, columns and declared types are pinned in `tests/public_schema.snap`,
//! one section per schema version, appended and never edited. A column
//! changed without a new version fails here.

mod common;

use common::fixture;
use git_evidence::PUBLIC_SCHEMA;
use std::collections::BTreeSet;
use std::fmt::Write as _;

fn exported(lib: &str) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    let mut previous = "";
    let mut statement = String::new();
    for line in lib.lines() {
        let trimmed = line.trim();
        if statement.is_empty() {
            if trimmed.starts_with("pub use ") {
                statement.push_str(trimmed);
            } else if trimmed == "pub mod testing;" {
                if previous != "#[cfg(feature = \"testing\")]" {
                    return Err("`testing` must stay behind its feature".into());
                }
                names.insert("testing".into());
            } else if trimmed.starts_with("pub ") {
                return Err(format!("a public item other than a re-export: {trimmed}"));
            }
        } else {
            statement.push(' ');
            statement.push_str(trimmed);
        }
        if !statement.is_empty() && statement.ends_with(';') {
            let path = &statement["pub use ".len()..statement.len() - 1];
            let list = match path.split_once("::{") {
                Some((_, rest)) => rest.trim_end_matches('}').to_owned(),
                None => path.rsplit("::").next().unwrap_or(path).to_owned(),
            };
            for item in list.split(',').map(str::trim).filter(|i| !i.is_empty()) {
                let name = item.rsplit(" as ").next().unwrap_or(item);
                names.insert(name.to_owned());
            }
            statement.clear();
        }
        previous = trimmed;
    }
    Ok(names)
}

#[test]
fn the_crate_root_exports_exactly_the_checked_in_surface() {
    let lib = include_str!("../src/lib.rs");
    let pinned: BTreeSet<String> = include_str!("public-surface.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    let actual = exported(lib).expect("lib.rs declares only re-exports");
    let added: Vec<_> = actual.difference(&pinned).collect();
    let removed: Vec<_> = pinned.difference(&actual).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "the public surface changed; update tests/public-surface.txt deliberately.\n\
         exported but not listed: {added:?}\nlisted but not exported: {removed:?}"
    );
}

#[test]
fn the_reader_refuses_what_it_cannot_pin() {
    assert!(exported("pub mod ask;").is_err());
    assert!(exported("pub fn helper() {}").is_err());
    assert!(exported("pub(crate) mod ask;\npub mod testing;").is_err());
    assert_eq!(
        exported("pub use a::{B, c as D};\npub use e::F;").expect("re-exports"),
        ["B", "D", "F"].into_iter().map(String::from).collect()
    );
}

/// The public views' columns, as `PRAGMA table_info` gives them, one line
/// per view in the order `about.public_views` lists them.
fn actual() -> String {
    let f = fixture(1);
    let cell = |rows: &git_evidence::Rows, row: usize, column: usize| {
        rows.rows[row][column].cell().unwrap_or_default()
    };
    let about = f
        .evidence
        .sql("SELECT public_views FROM about")
        .expect("about");
    let mut out = String::new();
    for view in cell(&about, 0, 0).split(' ') {
        let columns = f
            .evidence
            .sql(&format!(
                "SELECT name, type FROM pragma_table_info('{view}') ORDER BY cid"
            ))
            .expect("table_info");
        assert!(!columns.rows.is_empty(), "{view} is not a view");
        let columns: Vec<String> = (0..columns.rows.len())
            .map(|row| format!("{} {}", cell(&columns, row, 0), cell(&columns, row, 1)))
            .collect();
        writeln!(out, "{view}: {}", columns.join(", ")).expect("to a String");
    }
    out
}

/// The snapshot's sections: `## schema N` and the lines under it.
fn sections(snapshot: &str) -> Vec<(u32, String)> {
    let mut sections: Vec<(u32, String)> = Vec::new();
    for line in snapshot.lines() {
        if let Some(version) = line.strip_prefix("## schema ") {
            sections.push((
                version.trim().parse().expect("a version number"),
                String::new(),
            ));
        } else if let Some((_, body)) = sections.last_mut()
            && !line.trim().is_empty()
        {
            body.push_str(line);
            body.push('\n');
        }
    }
    sections
}

#[test]
fn the_public_views_are_the_snapshot_of_this_schema_version() {
    let snapshot = sections(include_str!("public_schema.snap"));
    let versions: Vec<u32> = snapshot.iter().map(|(v, _)| *v).collect();
    assert_eq!(
        versions,
        (1..=PUBLIC_SCHEMA).collect::<Vec<_>>(),
        "one section per schema version, in order, ending at PUBLIC_SCHEMA"
    );
    let pinned = &snapshot.last().expect("a section").1;
    assert_eq!(
        &actual(),
        pinned,
        "the public views changed: bump PUBLIC_SCHEMA and append a section to \
         tests/public_schema.snap (never edit an earlier one)"
    );
}

#[test]
fn the_reader_finds_each_section() {
    assert_eq!(
        sections("# note\n## schema 1\na: x INT\n\n## schema 2\na: x INT, y TEXT\n"),
        vec![
            (1, "a: x INT\n".to_owned()),
            (2, "a: x INT, y TEXT\n".to_owned())
        ]
    );
}
