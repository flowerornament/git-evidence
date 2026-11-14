//! The run of kind `tool` that records a bench (`--returned`): its open,
//! whose parameters keep only like figures in one variant, and its outcomes,
//! one per verb and one for the store's size.

use super::{Bench, BenchError, BenchFigure, BenchReport, BenchStore, BenchVerb};
use git_evidence::{CommitId, Key, Kind, Machine, MetricName, ProducerName};
use git_evidence::{Measure, Metric, Open, Outcome, Reason};

/// The build profile of this binary, the instrument measuring itself.
const PROFILE: &str = if cfg!(debug_assertions) {
    "debug"
} else {
    "release"
};

/// The kind a bench's run is recorded as: `tool`.
#[must_use]
pub fn bench_kind() -> Kind {
    Kind::parse("tool").expect("`tool` is a valid kind")
}

impl Bench<'_> {
    /// The `open` of the run that records this bench about `commit`, the code
    /// whose git-evidence was measured, on `machine`. Its parameters say which
    /// store (and, generated, its size) and the instrument, so only like
    /// figures share a variant.
    ///
    /// # Panics
    /// Never: every name is a constant that parses.
    #[must_use]
    pub fn open(&self, commit: CommitId, machine: &Machine) -> Open {
        let metric = |name| MetricName::parse(name).expect("a metric name");
        let open = Open::new(
            commit,
            ProducerName::parse("git-evidence").expect("a producer name"),
        )
        .version(env!("CARGO_PKG_VERSION"))
        .harness("git-evidence-bench/1")
        .machine(machine.clone())
        // The instrument, so every figure names it: the wall clock around one
        // process, its start included, the binary's compile not, and the
        // build profile it was compiled in.
        .param("clock", "wall")
        .param("process_start", "included")
        .param("compile", "excluded")
        .param("profile", PROFILE)
        .param("repeats", self.repeats.get().to_string())
        .metric(metric("cold_ms"), Metric::lower("ms"))
        .metric(metric("warm_ms"), Metric::lower("ms"))
        .metric(metric("landings"), Metric::new("{landing}", None))
        .metric(metric("runs"), Metric::new("{run}", None))
        .keys(
            BenchVerb::ALL
                .iter()
                .map(|verb| verb.name())
                .chain(["store"])
                .map(|key| Key::parse(key).expect("a key")),
        );
        match self.store {
            BenchStore::Generated(size) => open
                .param("store", "generated")
                .param("landings", size.landings.get().to_string())
                .param("runs", size.runs.to_string()),
            BenchStore::Existing(_) => open.param("store", "existing"),
        }
    }
}

impl BenchReport {
    /// Each verb's outcome, keyed by its name, and the store's size under
    /// `store`: the results of the run [`Bench::open`] opens. A verb with a
    /// failed figure is refused `answer_failed`, naming why.
    ///
    /// # Errors
    /// A figure that is not a finite number.
    pub fn outcomes(&self) -> Result<Vec<(Key, Outcome)>, BenchError> {
        let mut outcomes = Vec::new();
        for verb in BenchVerb::ALL {
            let mut measures = Vec::new();
            let mut failed = None;
            for timing in self.timings.iter().filter(|t| t.verb == verb) {
                match &timing.took {
                    BenchFigure::Measured(sample) => {
                        let mut measure = Measure::new(
                            MetricName::parse(timing.temperature.metric())?,
                            sample.median_ms,
                        )?;
                        measure.n = Some(sample.n);
                        measure.mad = Some(sample.mad_ms);
                        measures.push(measure);
                    }
                    BenchFigure::Failed(why) => failed = failed.or(Some(why.clone())),
                }
            }
            let outcome = match failed {
                Some(why) => Outcome::Refused(Reason::new("answer_failed").detail(why)),
                None => Outcome::Measured(measures),
            };
            outcomes.push((Key::parse(verb.name())?, outcome));
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a store's count, far below 2^52"
        )]
        let size = vec![
            Measure::new(MetricName::parse("landings")?, self.landings as f64)?,
            Measure::new(MetricName::parse("runs")?, self.runs as f64)?,
        ];
        outcomes.push((Key::parse("store")?, Outcome::Measured(size)));
        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::StoreSize;
    use std::num::NonZeroU32;

    #[test]
    fn the_open_declares_every_verb_and_the_store_key() {
        let bench = Bench {
            store: BenchStore::Generated(StoreSize::TARGET),
            repeats: NonZeroU32::new(3).expect("3"),
        };
        let open = bench.open(
            CommitId::parse(&"a".repeat(40)).expect("commit"),
            &Machine::parse("laptop").expect("machine"),
        );
        let keys: Vec<_> = open.keys.iter().flatten().map(Key::as_str).collect();
        assert_eq!(keys, ["overview", "trend", "sql", "record", "store"]);
        assert_eq!(open.suite.params["landings"], "5000");
        assert_eq!(open.suite.params["runs"], "10000");
        assert_eq!(open.suite.params["store"], "generated");
        assert_eq!(open.suite.params["clock"], "wall");
        assert_eq!(open.suite.params["profile"], PROFILE);
    }
}
