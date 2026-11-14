//! The crate README (shown on crates.io) opens with the same paragraphs as
//! the repository's root README. The one allowed difference is that the crate
//! README links into the repository by absolute URL.

/// Where the crate README's links into the repository start.
const REPOSITORY: &str = "https://github.com/flowerornament/git-evidence/blob/main/";

/// The paragraphs under the title, up to the first heading or code block.
fn opening(readme: &str) -> String {
    readme
        .lines()
        .skip_while(|line| !line.starts_with("# "))
        .skip(1)
        .take_while(|line| !line.starts_with('#') && !line.starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[test]
fn the_crate_readme_opens_as_the_root_readme_does() {
    // A packaged crate has no workspace, and no root README to compare with.
    if !std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml")).exists() {
        return;
    }
    let root = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"))
        .expect("the root README");
    let ours = opening(include_str!("../README.md")).replace(REPOSITORY, "");
    let theirs = opening(&root);
    assert!(
        theirs.lines().count() >= 5,
        "the root README's opening was found: {theirs}"
    );
    assert_eq!(ours, theirs, "the two READMEs' opening paragraphs differ");
}
