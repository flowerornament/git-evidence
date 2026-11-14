//! The names git-process exports, pinned in the checked-in list
//! `tests/public-surface.txt`: an export added, removed or renamed without
//! updating the list fails here, and so does a public item at the crate root
//! other than a `pub use` re-export or the feature-gated `testing` module. The
//! names that exist only with the `testing` feature (`StartFailureLog`, the
//! `testing` module and what it declares) are pinned in the list's own
//! section, so a change to them is as visible as a change to the root.
//!
//! A text check of `src/lib.rs` and `src/testing.rs` suffices because every
//! other module is private. (`cargo public-api` would read rustdoc JSON, which
//! needs a nightly toolchain the pinned shell does not carry.) The reader is
//! git-evidence's own (`crates/git-evidence/tests/contract.rs`); the two
//! crates are packaged apart, so each carries its copy.

use std::collections::BTreeSet;

/// The header of the list's section of names that exist only with the
/// `testing` feature.
const TESTING_SECTION: &str = "[feature testing]";

/// A public name, and whether it exists only with the `testing` feature.
type Name = (String, bool);

/// The names a file declares public at its top level, each marked gated when
/// the whole file is (`gated`) or a `#[cfg(...)]` naming the `testing` feature
/// precedes it. With `items`, a top-level `pub fn`, `const`, `struct`, … is a
/// name; without, it is refused, and only re-exports and a gated
/// `pub mod testing;` may be public.
fn declared(source: &str, gated: bool, items: bool) -> Result<BTreeSet<Name>, String> {
    let mut names = BTreeSet::new();
    let mut previous = "";
    let mut statement = String::new();
    let mut statement_gated = false;
    for line in source.lines() {
        let trimmed = line.trim();
        let behind_feature =
            gated || (previous.starts_with("#[cfg(") && previous.contains("feature = \"testing\""));
        if statement.is_empty() {
            if trimmed.starts_with("pub use ") && (!items || line.starts_with("pub use ")) {
                statement.push_str(trimmed);
                statement_gated = behind_feature;
            } else if !items && trimmed == "pub mod testing;" {
                if !behind_feature {
                    return Err("`testing` must stay behind its feature".into());
                }
                names.insert(("testing".into(), true));
            } else if !items && trimmed.starts_with("pub ") {
                return Err(format!("a public item other than a re-export: {trimmed}"));
            } else if items && line.starts_with("pub ") {
                names.insert((item_name(trimmed)?, behind_feature));
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
                names.insert((name.to_owned(), statement_gated));
            }
            statement.clear();
        }
        previous = trimmed;
    }
    Ok(names)
}

/// The name a top-level `pub` item declares: `pub const fn note(` is `note`.
fn item_name(line: &str) -> Result<String, String> {
    let words: Vec<&str> = line.split_whitespace().skip(1).collect();
    let mut at = 0;
    // Qualifiers before the keyword: `const fn`, `unsafe fn`, `async fn`.
    while matches!(words.get(at), Some(&("const" | "unsafe" | "async")))
        && matches!(words.get(at + 1), Some(&("fn" | "unsafe" | "async")))
    {
        at += 1;
    }
    let keyword = matches!(
        words.get(at),
        Some(&("fn" | "const" | "static" | "struct" | "enum" | "trait" | "type" | "mod" | "union"))
    );
    let name: String = words
        .get(at + 1)
        .copied()
        .unwrap_or_default()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if !keyword || name.is_empty() {
        return Err(format!("a public item the reader cannot name: {line}"));
    }
    Ok(name)
}

/// The crate's public names: the root's, and the `testing` module's as
/// `testing::<name>`, every one of those gated.
fn exported(lib: &str, testing: &str) -> Result<BTreeSet<Name>, String> {
    let mut names = declared(lib, false, false)?;
    for (name, _) in declared(testing, true, true)? {
        names.insert((format!("testing::{name}"), true));
    }
    Ok(names)
}

/// The checked-in list: one name per line, `#` comments, and the names after
/// the `[feature testing]` header gated.
fn pinned(list: &str) -> BTreeSet<Name> {
    let mut gated = false;
    let mut names = BTreeSet::new();
    for line in list.lines().map(str::trim) {
        if line == TESTING_SECTION {
            gated = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            names.insert((line.to_owned(), gated));
        }
    }
    names
}

#[test]
fn the_crate_root_exports_exactly_the_checked_in_surface() {
    let actual = exported(
        include_str!("../src/lib.rs"),
        include_str!("../src/testing.rs"),
    )
    .expect("lib.rs declares only re-exports");
    let pinned = pinned(include_str!("public-surface.txt"));
    let spell = |(name, gated): &Name| {
        if *gated {
            format!("{name} (under {TESTING_SECTION})")
        } else {
            name.clone()
        }
    };
    let added: Vec<_> = actual.difference(&pinned).map(spell).collect();
    let removed: Vec<_> = pinned.difference(&actual).map(spell).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "the public surface changed; update tests/public-surface.txt deliberately.\n\
         exported but not listed: {added:?}\nlisted but not exported: {removed:?}"
    );
}

#[test]
fn the_reader_refuses_what_it_cannot_pin() {
    assert!(exported("pub mod ask;", "").is_err());
    assert!(exported("pub fn helper() {}", "").is_err());
    assert!(exported("pub(crate) mod ask;\npub mod testing;", "").is_err());
    assert!(exported("", "pub impl X {}").is_err());
    let names = |list: &[(&str, bool)]| -> BTreeSet<Name> {
        list.iter().map(|&(n, g)| (n.to_owned(), g)).collect()
    };
    assert_eq!(
        exported(
            "pub use a::{B, c as D};\npub use e::F;\n#[cfg(feature = \"testing\")]\npub mod testing;",
            "pub use x::{Y,\n    z};\npub const fn w() {}\nimpl S {\n    pub fn method() {}\n}\npub struct S;",
        )
        .expect("re-exports and items"),
        names(&[
            ("B", false),
            ("D", false),
            ("F", false),
            ("testing", true),
            ("testing::S", true),
            ("testing::Y", true),
            ("testing::w", true),
            ("testing::z", true),
        ])
    );
    assert_eq!(
        pinned("# a comment\nA\n\n[feature testing]\ntesting\n"),
        names(&[("A", false), ("testing", true)])
    );
}
