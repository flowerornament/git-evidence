//! The manual: `git-evidence.1` and one `git-evidence-<verb>.1` per verb,
//! rendered from the command line's grammar (`args.rs`), so a page can never
//! say what `--help` does not. The hidden `manual DIR` verb writes them; put
//! in a `man1` directory on the manual path, they are what `git evidence
//! --help` shows: Git answers `git <command> --help` with
//! `man git-<command>`.
//!
//! The exit status is the one fact the grammar cannot carry. It is declared
//! here once and shown both by `git-evidence --help` (after the verbs) and as
//! the root page's EXIT STATUS section.

use super::args::Cli;
use clap::CommandFactory as _;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// Each exit code and what it means: the one statement of them, which the
/// README points to.
const EXIT_STATUS: [(&str, &str); 3] = [
    (
        "0",
        "The rows were printed: the header, then one line per row, or the header \
         alone when no row matched.",
    ),
    (
        "1",
        "The command could not run: a malformed command, a revision that does not \
         resolve, no trunk, a store it cannot read, or an error of the tool. \
         Standard error says why; standard output is empty.",
    ),
    (
        "2",
        "Nothing is recorded for what the command named (a key, a metric, a run, a \
         commit with no run, a range end that is not a landing); standard error \
         names it and the nearest recorded names. Also a refused record, fetch or \
         push, and an audit with findings.",
    ),
];

/// Where the rest of the reference lives.
const REFERENCE: &str = "The git-evidence crate's README is the full reference: the model, \
     the event lines, the Git config evidence.* settings and the storage.";

/// What `sql --help` adds below its options: the public views by name (from
/// the library's own list), how to see a view's columns, and questions
/// worked as queries.
pub fn sql_help() -> String {
    let mut text = String::from("Public views:\n");
    for view in git_evidence::PUBLIC_VIEWS {
        writeln!(text, "  {view}").expect("writing to a String cannot fail");
    }
    text.push_str(SQL_QUESTIONS);
    text
}

/// Questions worked as queries, for `sql --help`.
const SQL_QUESTIONS: &str = "
The views by name, and the index's versions: git evidence sql \"SELECT * FROM about\"
A view's columns, one row each:
  git evidence sql \"SELECT name, type FROM pragma_table_info('measures')\"

The metrics recorded, with unit and direction: git evidence keys --metric 'GLOB'

Worked questions:
  -- the newest landing with a complete run, per kind and machine
  SELECT kind, machine, max(landing) AS landing FROM runs
   WHERE ended = 'complete' GROUP BY kind, machine
  -- one key's metric in one run
  SELECT value, unit FROM measures WHERE run = 'RUN' AND key = 'KEY' AND metric = 'METRIC'
  -- metrics side by side: a chain of LEFT JOINs on measures, one per column
  SELECT a.run, a.value AS a, b.value AS b FROM measures a
    LEFT JOIN measures b ON b.run = a.run AND b.key = 'KEY_B' AND b.metric = a.metric
   WHERE a.key = 'KEY_A' AND a.metric = 'METRIC'
";

/// What `--help` prints after the verbs: the exit status and the reference.
pub fn after_help() -> String {
    let mut text = String::from("Exit status:\n");
    for (code, meaning) in EXIT_STATUS {
        writeln!(text, "  {code}  {meaning}").expect("writing to a String cannot fail");
    }
    text.push('\n');
    text.push_str(REFERENCE);
    text
}

/// Writes `git-evidence.1` and one page per visible verb into `dir`, which
/// must exist; a page already there is overwritten.
pub fn write(dir: &Path) -> std::io::Result<()> {
    // Git's `help` verb is clap's; the pages are the manual's own.
    let mut command = Cli::command().disable_help_subcommand(true);
    command.build();
    for verb in command.get_subcommands().filter(|verb| !verb.is_hide_set()) {
        clap_mangen::Man::new(verb.clone()).generate_to(dir)?;
    }
    let root = clap_mangen::Man::new(command);
    let mut page = std::fs::File::create(dir.join(root.get_filename()))?;
    root.render_title(&mut page)?;
    root.render_name_section(&mut page)?;
    root.render_synopsis_section(&mut page)?;
    root.render_description_section(&mut page)?;
    root.render_options_section(&mut page)?;
    root.render_subcommands_section(&mut page)?;
    let mut sections = roff::Roff::new();
    sections.control("SH", ["EXIT STATUS"]);
    for (code, meaning) in EXIT_STATUS {
        sections
            .control("TP", [] as [&str; 0])
            .text([roff::bold(code)])
            .text([roff::roman(meaning)]);
    }
    sections
        .control("SH", ["SEE ALSO"])
        .text([roff::roman(REFERENCE)])
        .control("PP", [] as [&str; 0])
        .text([
            roff::bold("git-notes"),
            roff::roman("(1), "),
            roff::bold("git-fetch"),
            roff::roman("(1), "),
            roff::bold("git-push"),
            roff::roman("(1)"),
        ]);
    sections.to_writer(&mut page)?;
    root.render_version_section(&mut page)?;
    page.flush()
}
