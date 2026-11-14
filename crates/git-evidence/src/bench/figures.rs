//! What a bench measured: for each verb and temperature, a sample of wall
//! times or why the command could not answer.

use git_evidence::TidyRows;
use serde::Serialize;

/// One command a bench times. Its name is the key in the bench's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchVerb {
    /// `git evidence` with no verb.
    Overview,
    /// `git evidence trend` of the first metric and key the newest run measured.
    Trend,
    /// `git evidence sql` of a reference query.
    Sql,
    /// `git evidence record` of one perf run of 16 keys on the newest landing.
    Record,
}

impl BenchVerb {
    /// Every verb, in the order a bench times them.
    pub const ALL: [Self; 4] = [Self::Overview, Self::Trend, Self::Sql, Self::Record];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Trend => "trend",
            Self::Sql => "sql",
            Self::Record => "record",
        }
    }
}

/// Whether the index existed when the command started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchTemperature {
    /// No index: the command built it from the notes.
    Cold,
    /// The index was current, and the command had run once before.
    Warm,
}

impl BenchTemperature {
    /// The metric this temperature's figure is recorded as.
    #[must_use]
    pub const fn metric(self) -> &'static str {
        match self {
            Self::Cold => "cold_ms",
            Self::Warm => "warm_ms",
        }
    }
}

/// Wall times of one command, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BenchSample {
    pub median_ms: f64,
    /// Median absolute deviation from the median; 0 for one time.
    pub mad_ms: f64,
    pub n: u32,
}

/// What timing one command gave.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchFigure {
    Measured(BenchSample),
    /// The command could not answer (exit 1, or it did not start): why.
    Failed(String),
}

/// One command's figure at one temperature.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchTiming {
    pub verb: BenchVerb,
    pub temperature: BenchTemperature,
    pub took: BenchFigure,
}

/// Which store a report measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchOrigin {
    Generated,
    Existing,
}

/// What a bench measured.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BenchReport {
    pub origin: BenchOrigin,
    /// Landings on the measured store's trunk.
    pub landings: u64,
    /// Runs in the measured store before the record figures added theirs.
    pub runs: u64,
    pub repeats: u32,
    pub timings: Vec<BenchTiming>,
}

impl TidyRows for BenchReport {
    fn header(&self) -> Vec<String> {
        [
            "verb", "metric", "value", "unit", "n", "mad", "outcome", "detail",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    fn rows(&self) -> Vec<Vec<Option<String>>> {
        self.timings
            .iter()
            .map(|timing| {
                let metric = Some(timing.temperature.metric().to_owned());
                let verb = Some(timing.verb.name().to_owned());
                match &timing.took {
                    BenchFigure::Measured(sample) => vec![
                        verb,
                        metric,
                        Some(sample.median_ms.to_string()),
                        Some("ms".to_owned()),
                        Some(sample.n.to_string()),
                        Some(sample.mad_ms.to_string()),
                        Some("measured".to_owned()),
                        None,
                    ],
                    BenchFigure::Failed(why) => vec![
                        verb,
                        metric,
                        None,
                        None,
                        None,
                        None,
                        Some("failed".to_owned()),
                        Some(why.clone()),
                    ],
                }
            })
            .collect()
    }
}
