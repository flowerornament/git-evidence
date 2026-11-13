//! Event lines as they are written in notes.
//!
//! A note on a tree is JSON lines; each line is one [`EventLine`]. A run is the
//! fold of its lines ordered by `seq`, in the grammar
//! `open · declare? · result* · close?`. Producers declare fields; nothing here
//! is a hash a producer must compute. Envelopes evolve additively: readers never
//! refuse unknown fields.
//!
//! A run declares each metric it reports once, with its unit and direction
//! (`suite.metrics`, or `declare.metrics` when the population is learned late);
//! a measure carries only its metric, value and spread, so a number's unit is
//! never guessed.

use crate::format::id::{CommitId, Key, MetricName, ProducerName, RunId, Sha256};
/// An event's `time` is the one wire spelling of a UTC time, owned by `git-process`.
pub use git_process::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The envelope version this crate writes.
pub const ENVELOPE_VERSION: u32 = 2;

/// The prefix of the metrics the index derives (verdict durations); a run may
/// not declare one.
pub const DERIVED_PREFIX: &str = "verdict.";

/// The tool that produced the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Producer {
    pub name: ProducerName,
    /// Absent when the source did not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// What is being asked: the harness and its operative parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Suite {
    /// The harness-semantics version: bumped when a change alters what values mean.
    #[serde(default)]
    pub harness: String,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    /// The metrics this run reports, each with its unit and direction.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<MetricName, Metric>,
}

/// What a metric's numbers mean: its unit and which way is better. Part of a
/// value's meaning, so two values are compared only when these agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metric {
    /// A UCUM unit: `ns`, `us`, `By`, `1`, `{count}`.
    pub unit: String,
    /// Which way is an improvement; absent when neither is (a count that is
    /// neither good nor bad, such as how many regions a plan has).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub better: Option<Direction>,
}

/// Where a run executed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    /// The machine's declared name: `bench`, `laptop`. Part of the variant.
    #[serde(default)]
    pub machine: String,
    /// Operative parameters of the environment: `host_role`, pinned cores.
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// What the machine was (CPU, OS, kernel): provenance, never part of the variant.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub descriptor: BTreeMap<String, serde_json::Value>,
}

/// The build a run executed: its profile is part of the question, the rest is
/// what a comparison measures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Build {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// The digest of the binary that ran, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Sha256>,
}

/// Target and flags: a debug and a release build answer different questions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
}

/// The payload of `open`, written before any work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Open {
    /// The commit measured; its tree is the note's key.
    pub commit: CommitId,
    pub producer: Producer,
    #[serde(default)]
    pub suite: Suite,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub build: Build,
    /// The keys, when known before the work; else one `declare` names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<Key>>,
    /// The original record this run observes, by an identity unique to that
    /// record (an id, an immutable path or a digest), never by where a copy was
    /// found. A producer measuring afresh names a new record each time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Open {
    /// An open about `commit`, by `producer` under `harness`, on `machine`; add
    /// keys, metrics, parameters and a build through the public fields.
    #[must_use]
    pub fn new(
        commit: CommitId,
        producer: ProducerName,
        harness: &str,
        machine: &crate::format::id::Machine,
    ) -> Self {
        Self {
            commit,
            producer: Producer {
                name: producer,
                version: None,
            },
            suite: Suite {
                harness: harness.to_owned(),
                ..Suite::default()
            },
            environment: Environment {
                machine: machine.as_str().to_owned(),
                ..Environment::default()
            },
            build: Build::default(),
            keys: None,
            source: None,
        }
    }
}

/// A number that JSON cannot hold: NaN or an infinity.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("{0} is not a finite number; JSON has no NaN or infinity")]
pub struct NotFinite(pub f64);

/// Which way is better for a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Lower,
    Higher,
}

/// One summarised metric of a measured key; its unit and direction are the
/// run's declaration of the metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub metric: MetricName,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Median absolute deviation, unscaled, in the metric's declared unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mad: Option<f64>,
}

impl Measure {
    /// A value of a declared metric, without spread.
    ///
    /// # Errors
    /// [`NotFinite`] for NaN or an infinity, when it is built rather than
    /// when it is written.
    pub fn new(metric: MetricName, value: f64) -> Result<Self, NotFinite> {
        if !value.is_finite() {
            return Err(NotFinite(value));
        }
        Ok(Self {
            metric,
            value,
            n: None,
            mad: None,
        })
    }
}

/// A verdict's status. Flaky is derived from tries, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictStatus {
    Pass,
    Fail,
    Timeout,
}

impl VerdictStatus {
    /// Every status, so derived per-status metrics are named from one list.
    pub const ALL: [Self; 3] = [Self::Pass, Self::Fail, Self::Timeout];

    /// The status as written in events.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Timeout => "timeout",
        }
    }
}

/// One try at a verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Try {
    pub status: VerdictStatus,
    /// Wall time of the try, in milliseconds; absent when not recorded.
    /// `record` refuses a value that is not finite or is negative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    /// CPU time of the try, in milliseconds: user plus system time of the
    /// process that ran it and the descendants it waited for. Absent when not
    /// recorded, never zero for unknown; `record` refuses a value that is not
    /// finite or is negative. Parallel work can take more CPU than wall time,
    /// so it is not bounded by `duration_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<f64>,
}

/// Reads one time, in milliseconds, from a try; `None` when it was not recorded.
pub(crate) type TimeOf = fn(&Try) -> Option<f64>;

impl Try {
    /// The times a try may carry, by field name: the index sums each across a
    /// verdict's tries into the derived metric `verdict.<status>.<name>`.
    pub(crate) const TIMES: [(&'static str, TimeOf); 2] =
        [("duration_ms", |t| t.duration_ms), ("cpu_ms", |t| t.cpu_ms)];
}

/// A pass/fail outcome with its tries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub status: VerdictStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tries: Vec<Try>,
}

/// Why a key has no value: a reason code and what was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Reason {
    #[must_use]
    pub fn new(reason: &str, detail: Option<&str>) -> Self {
        Self {
            reason: reason.to_owned(),
            detail: detail.map(str::to_owned),
        }
    }
}

/// A key's result: a value, a verdict, or why there is none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Measured(Vec<Measure>),
    Verdict(Verdict),
    /// No value; retrying may succeed.
    Refused(Reason),
    /// No value, and none is possible at this code.
    Skipped(Reason),
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// Every declared key was answered.
    Complete,
    /// The run stopped; every unanswered key is refused with this reason.
    Refused(Reason),
}

/// The events of the run grammar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Open(Box<Open>),
    /// The keys, the metrics and what was built, once the producer knows them.
    Declare {
        /// Absent when `open` already named the keys.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Vec<Key>>,
        /// Metrics learned late; they extend the run's declarations, never redefine one.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metrics: BTreeMap<MetricName, Metric>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build: Option<Build>,
    },
    Result {
        key: Key,
        outcome: Outcome,
        /// The code site that produced the value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
    },
    Close {
        end: End,
        /// Readings taken while the run executed (load): provenance only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conditions: Option<serde_json::Value>,
    },
}

/// One line of a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventLine {
    pub v: u32,
    pub run: RunId,
    /// The event's position within its run, from 0. The fold orders by it, so a
    /// note's line order never matters.
    pub seq: u32,
    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`, the one form that sorts as text, as the
    /// line spells it: the fold and `record` read it as a [`Timestamp`], so a
    /// malformed time is refused naming its run and seq.
    pub time: String,
    #[serde(flatten)]
    pub event: Event,
}

/// Sorted-key compact JSON, for the digests the index derives.
///
/// # Panics
/// Never: every value passed is plain data that `serde_json` can represent.
#[must_use]
pub(crate) fn canonical_json(value: &impl Serialize) -> Vec<u8> {
    let tree = serde_json::to_value(value).expect("plain data serializes to a JSON value");
    serde_json::to_vec(&tree).expect("a JSON value serializes")
}

/// The variant: what must match for two outcomes of one key to be compared
/// by `diff`. Derived by the index from the open as completed by the
/// run's `declare`, never stored or computed by producers.
#[must_use]
pub(crate) fn variant_of(kind: &str, open: &Open) -> Sha256 {
    Sha256::of(&canonical_json(&serde_json::json!({
        "kind": kind,
        "producer": open.producer.name,
        "harness": open.suite.harness,
        "suite": open.suite.params,
        "profile": open.build.profile,
        "machine": open.environment.machine,
        "environment": open.environment.params,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Open {
        Open {
            commit: CommitId::parse(&"c".repeat(40)).expect("commit"),
            producer: Producer {
                name: ProducerName::parse("bench-suite").expect("producer"),
                version: Some("0.41.0".into()),
            },
            suite: Suite {
                harness: "bench-suite/7".into(),
                ..Suite::default()
            },
            environment: Environment {
                machine: "bench".into(),
                ..Environment::default()
            },
            build: Build::default(),
            keys: Some(vec![Key::parse("parse_large_d64").expect("key")]),
            source: None,
        }
    }

    #[test]
    fn event_lines_round_trip_in_the_spec_shape() {
        let line = EventLine {
            v: ENVELOPE_VERSION,
            run: RunId::parse("01JA8Z3K4M0000000000000000").expect("run"),
            seq: 3,
            time: "2026-10-08T03:10:02.123Z".into(),
            event: Event::Result {
                key: Key::parse("swap_quality").expect("key"),
                outcome: Outcome::Refused(Reason::new("noisy_host", None)),
                location: None,
            },
        };
        let text = serde_json::to_string(&line).expect("json");
        assert!(text.contains("\"event\":\"result\""), "{text}");
        assert!(
            text.contains("\"refused\":{\"reason\":\"noisy_host\"}"),
            "{text}"
        );
        assert_eq!(
            serde_json::from_str::<EventLine>(&text).expect("parse"),
            line
        );
    }

    #[test]
    fn a_try_without_cpu_time_reads_as_absent_and_writes_back_byte_for_byte() {
        let old = r#"{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":3,"time":"2026-10-08T03:10:02.123Z","event":"result","key":"lexer/tokens","outcome":{"verdict":{"status":"pass","tries":[{"status":"fail","duration_ms":812.0},{"status":"pass","duration_ms":790.0}]}}}"#;
        let line: EventLine = serde_json::from_str(old).expect("an old line");
        let Event::Result {
            outcome: Outcome::Verdict(verdict),
            ..
        } = &line.event
        else {
            panic!("verdict")
        };
        assert!(verdict.tries.iter().all(|t| t.cpu_ms.is_none()));
        assert_eq!(serde_json::to_string(&line).expect("json"), old);
        let new = old.replace(
            r#""duration_ms":790.0}"#,
            r#""duration_ms":790.0,"cpu_ms":2400.5}"#,
        );
        let line: EventLine = serde_json::from_str(&new).expect("a line with cpu_ms");
        let Event::Result {
            outcome: Outcome::Verdict(verdict),
            ..
        } = &line.event
        else {
            panic!("verdict")
        };
        assert_eq!(verdict.tries[1].cpu_ms, Some(2400.5));
        assert_eq!(serde_json::to_string(&line).expect("json"), new);
    }

    #[test]
    fn readers_tolerate_unknown_fields() {
        let text = r#"{"v":2,"run":"01JA8Z3K4M0000000000000000","seq":0,"time":"x","event":"close","end":"complete","future":true}"#;
        let line: EventLine = serde_json::from_str(text).expect("additive field");
        assert!(matches!(
            line.event,
            Event::Close {
                end: End::Complete,
                ..
            }
        ));
    }

    #[test]
    fn the_variant_ignores_the_artifact_toolchain_and_descriptor_but_not_the_profile() {
        let base = open();
        let mut same = base.clone();
        same.build.artifact = Some(Sha256::of(b"another binary"));
        same.build.toolchain = Some("1.95.0".into());
        same.environment
            .descriptor
            .insert("os".into(), "Linux 6.13".into());
        same.commit = CommitId::parse(&"d".repeat(40)).expect("commit");
        assert_eq!(variant_of("perf", &base), variant_of("perf", &same));
        let mut debug = base.clone();
        debug.build.profile = Some(Profile {
            target: None,
            flags: Some("debug".into()),
        });
        assert_ne!(variant_of("perf", &base), variant_of("perf", &debug));
        let mut other_machine = base.clone();
        other_machine.environment.machine = "laptop".into();
        assert_ne!(
            variant_of("perf", &base),
            variant_of("perf", &other_machine)
        );
        assert_ne!(variant_of("perf", &base), variant_of("gates", &base));
    }
}
