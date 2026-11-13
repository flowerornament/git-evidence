//! The command line prints the Rust API's rows: aligned columns by default,
//! `--csv` for a script, the same header either way; and its exit status says
//! printed (0), could not run (1) or nothing recorded for what was named (2).

#[path = "../common/mod.rs"]
mod common;

mod answers;
mod csv;
mod no_machine;
