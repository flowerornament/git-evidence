//! `git evidence bench`: the tool's own benchmark. It times the command line
//! answering on a store nobody else reads, in
//! a temporary directory: a store generated at a declared size, or a copy of
//! an existing store that borrows its objects (Git alternates) and copies its
//! evidence refs, so the store in use is only read, and its index is never
//! rebuilt or locked.
//!
//! ```text
//! store ─▶ cold   the overview, from no index: the rebuild every verb pays first
//!       ─▶ warm   overview · trend · sql, each once untimed, then R times
//!       ─▶ record R runs, each one perf run on the newest landing (after one untimed)
//!       ─▶ BenchReport ─┬─ aligned columns · --csv
//!                       └─ outcomes(): one run of kind `tool`, keyed by verb (`--returned`)
//! ```
//!
//! Each figure is wall time around one process of the binary measured, so it
//! is what a person typing the command waits for, process start included. A
//! warm figure is the median of R with its median absolute deviation.
//!
//! ```text
//! measure    Bench (a store, R) ─▶ run(program) ─▶ BenchReport; why it cannot: BenchError
//! store      the store measured (BenchStore): generated at a StoreSize, or a copy of an existing one
//! generate   the generated store of a declared size
//! figures    what a bench measured: per verb and temperature, a BenchSample or why it failed
//! tool_run   the run of kind `tool` that records a bench: its open, its outcomes
//! ```

mod figures;
mod generate;
mod measure;
mod stats;
mod store;
mod tool_run;

pub use figures::{
    BenchFigure, BenchOrigin, BenchReport, BenchSample, BenchTemperature, BenchTiming, BenchVerb,
};
pub use measure::{Bench, BenchError};
pub use store::{BenchStore, StoreSize};
pub use tool_run::bench_kind;
