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
//!
//! Each struct is built by its constructor and one builder method per thing
//! a caller sets. A builder for an optional field takes the value or an
//! `Option` of it, so a value that is not always known passes straight
//! through: `Try::new(status).cpu_ms(cpu)` with `cpu: Option<f64>`.

use crate::format::id::{CommitId, Key, Machine, MetricName, ProducerName, RunId, Sha256};
/// An event's `time` is the one wire spelling of a UTC time, owned by `git-process`.
pub use git_process::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The envelope version this crate writes.
pub const ENVELOPE_VERSION: u32 = 2;

/// The prefix of the metrics the index derives (verdict durations); a run may
/// not declare one.
pub const DERIVED_PREFIX: &str = "verdict.";

/// A value or an `Option` of it: what a builder for an optional field takes,
/// so a caller that has an `Option` (a CPU time not always measured) passes
/// it as it is, and one that has the value passes that.
///
/// Implemented for `f64` and `u32`, for the text types (`&str`, `String`,
/// `&String`, as `Optional<String>`), for [`Profile`] and [`Sha256`], and
/// for an `Option` of each. Sealed: this crate alone implements it.
pub trait Optional<T>: sealed::Sealed<T> {}

mod sealed {
    /// Turns a value or an `Option` of it into the `Option`.
    pub trait Sealed<T> {
        fn into_option(self) -> Option<T>;
    }
}

/// Implements [`Optional`] for a value and for an `Option` of it.
macro_rules! optional {
    ($target:ty: $($value:ty),+) => {$(
        impl Optional<$target> for $value {}
        impl sealed::Sealed<$target> for $value {
            fn into_option(self) -> Option<$target> {
                Some(self.into())
            }
        }
        impl Optional<$target> for Option<$value> {}
        impl sealed::Sealed<$target> for Option<$value> {
            fn into_option(self) -> Option<$target> {
                self.map(Into::into)
            }
        }
    )+};
}

optional!(f64: f64);
optional!(u32: u32);
optional!(String: String, &str, &String);
optional!(Profile: Profile);
optional!(Sha256: Sha256);

/// The `Option` a builder's argument stands for.
fn optional<T>(value: impl Optional<T>) -> Option<T> {
    sealed::Sealed::into_option(value)
}

/// The tool that produced the values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Producer {
    pub name: ProducerName,
    /// Absent when the source did not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Producer {
    /// A producer named `name`, its version unrecorded.
    #[must_use]
    pub const fn new(name: ProducerName) -> Self {
        Self {
            name,
            version: None,
        }
    }

    /// The producer with its version, or with none recorded for `None`.
    #[must_use]
    pub fn version(mut self, version: impl Optional<String>) -> Self {
        self.version = optional(version);
        self
    }
}

/// What is being asked: the harness and its operative parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl Suite {
    /// A suite naming no harness (`""` on the wire), with no parameters and
    /// no metrics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The suite under `harness`, the version of what its values mean.
    #[must_use]
    pub fn harness(mut self, harness: impl Into<String>) -> Self {
        self.harness = harness.into();
        self
    }

    /// The suite with one more operative parameter: `repeats`, a dataset.
    /// A parameter named again replaces the earlier value.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    /// The suite with one more metric, its unit and direction.
    #[must_use]
    pub fn metric(mut self, name: MetricName, metric: Metric) -> Self {
        self.metrics.insert(name, metric);
        self
    }
}

/// What a metric's numbers mean: its unit and which way is better. Part of a
/// value's meaning, so two values are compared only when these agree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Metric {
    /// A UCUM unit: `ns`, `us`, `By`, `1`, `{count}`.
    pub unit: String,
    /// Which way is an improvement; absent when neither is (a count that is
    /// neither good nor bad, such as how many regions a plan has).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub better: Option<Direction>,
}

impl Metric {
    /// A metric in `unit`, better in the direction given, or in neither.
    #[must_use]
    pub fn new(unit: impl Into<String>, better: Option<Direction>) -> Self {
        Self {
            unit: unit.into(),
            better,
        }
    }

    /// A metric in `unit` where lower is better: a time, a size.
    #[must_use]
    pub fn lower(unit: impl Into<String>) -> Self {
        Self::new(unit, Some(Direction::Lower))
    }

    /// A metric in `unit` where higher is better: a throughput.
    #[must_use]
    pub fn higher(unit: impl Into<String>) -> Self {
        Self::new(unit, Some(Direction::Higher))
    }
}

/// Where a run executed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl Environment {
    /// An environment naming no machine (`""` on the wire), with no
    /// parameters and no descriptor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The environment on `machine`.
    #[must_use]
    pub fn machine(mut self, machine: Machine) -> Self {
        self.machine = machine.into();
        self
    }

    /// The environment with one more operative parameter: `host_role`, a
    /// pinned core. A parameter named again replaces the earlier value.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    /// The environment with one more fact about what the machine was: `cpu`,
    /// `os`. Provenance only, never part of the variant.
    #[must_use]
    pub fn descriptor(
        mut self,
        name: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.descriptor.insert(name.into(), value.into());
        self
    }
}

/// The build a run executed: its profile is part of the question, the rest is
/// what a comparison measures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Build {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// The digest of the binary that ran, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Sha256>,
}

impl Build {
    /// A build with nothing recorded about it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The build with its profile, or with none for `None`.
    #[must_use]
    pub fn profile(mut self, profile: impl Optional<Profile>) -> Self {
        self.profile = optional(profile);
        self
    }

    /// The build with the toolchain that made it (`1.95.0`), or with none
    /// for `None`.
    #[must_use]
    pub fn toolchain(mut self, toolchain: impl Optional<String>) -> Self {
        self.toolchain = optional(toolchain);
        self
    }

    /// The build with the digest of the binary that ran, or with none for `None`.
    #[must_use]
    pub fn artifact(mut self, artifact: impl Optional<Sha256>) -> Self {
        self.artifact = optional(artifact);
        self
    }
}

/// Target and flags: a debug and a release build answer different questions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
}

impl Profile {
    /// A profile naming neither target nor flags.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The profile for `target` (`x86_64-unknown-linux-gnu`), or for none
    /// named with `None`.
    #[must_use]
    pub fn target(mut self, target: impl Optional<String>) -> Self {
        self.target = optional(target);
        self
    }

    /// The profile with its flags (`release`, `debug`), or with none named
    /// for `None`.
    #[must_use]
    pub fn flags(mut self, flags: impl Optional<String>) -> Self {
        self.flags = optional(flags);
        self
    }
}

/// The payload of `open`, written before any work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// An open about `commit`, by `producer`, naming nothing else: no
    /// harness and no machine (both `""` on the wire). Each builder below
    /// sets one thing; the keys and the build may wait for a `declare`.
    #[must_use]
    pub fn new(commit: CommitId, producer: ProducerName) -> Self {
        Self {
            commit,
            producer: Producer::new(producer),
            suite: Suite::new(),
            environment: Environment::new(),
            build: Build::new(),
            keys: None,
            source: None,
        }
    }

    /// Run by the producer's `version`, or with none recorded for `None`.
    #[must_use]
    pub fn version(mut self, version: impl Optional<String>) -> Self {
        self.producer = self.producer.version(version);
        self
    }

    /// Run under `harness`, the version of what its values mean.
    #[must_use]
    pub fn harness(mut self, harness: impl Into<String>) -> Self {
        self.suite.harness = harness.into();
        self
    }

    /// Run with one more operative parameter of the suite: `repeats`, a
    /// dataset. A parameter named again replaces the earlier value.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.suite = self.suite.param(name, value);
        self
    }

    /// Run declaring one more metric, its unit and direction.
    #[must_use]
    pub fn metric(mut self, name: MetricName, metric: Metric) -> Self {
        self.suite = self.suite.metric(name, metric);
        self
    }

    /// Run under `suite`. It replaces the whole suite, its harness, parameters
    /// and metrics included: set them on the [`Suite`], or call
    /// [`Self::harness`], [`Self::param`] or [`Self::metric`] after this.
    #[must_use]
    pub fn suite(mut self, suite: Suite) -> Self {
        self.suite = suite;
        self
    }

    /// Run on `machine`.
    #[must_use]
    pub fn machine(mut self, machine: Machine) -> Self {
        self.environment.machine = machine.into();
        self
    }

    /// Run in `environment`. It replaces the whole environment, its machine
    /// included: name the machine on the [`Environment`], or call
    /// [`Self::machine`] after this.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    /// Run on what was built, when it is known before the work; else a
    /// `declare` names it.
    #[must_use]
    pub fn build(mut self, build: Build) -> Self {
        self.build = build;
        self
    }

    /// Run answering these keys, when they are known before the work; else
    /// a `declare` names them.
    #[must_use]
    pub fn keys(mut self, keys: impl IntoIterator<Item = Key>) -> Self {
        self.keys = Some(keys.into_iter().collect());
        self
    }

    /// Run observing the original record `source`, named by an identity
    /// unique to it (an id, an immutable path or a digest); `None` for a run
    /// that measures afresh.
    #[must_use]
    pub fn source(mut self, source: impl Optional<String>) -> Self {
        self.source = optional(source);
        self
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
#[non_exhaustive]
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

    /// The value with how many samples it summarises, or with no count for `None`.
    #[must_use]
    pub fn n(mut self, n: impl Optional<u32>) -> Self {
        self.n = optional(n);
        self
    }

    /// The value with its spread: the median absolute deviation, unscaled,
    /// in the metric's unit, or with no spread for `None`. `record` refuses
    /// one that is not finite.
    #[must_use]
    pub fn mad(mut self, mad: impl Optional<f64>) -> Self {
        self.mad = optional(mad);
        self
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
#[non_exhaustive]
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
    /// A try that ended with `status`, its times unrecorded.
    #[must_use]
    pub const fn new(status: VerdictStatus) -> Self {
        Self {
            status,
            duration_ms: None,
            cpu_ms: None,
        }
    }

    /// The try with its wall time, in milliseconds, or unrecorded for `None`.
    #[must_use]
    pub fn duration_ms(mut self, ms: impl Optional<f64>) -> Self {
        self.duration_ms = optional(ms);
        self
    }

    /// The try with its CPU time, in milliseconds, or unrecorded for `None`
    /// (never zero for unknown).
    #[must_use]
    pub fn cpu_ms(mut self, ms: impl Optional<f64>) -> Self {
        self.cpu_ms = optional(ms);
        self
    }

    /// The times a try may carry, by field name: the index sums each across a
    /// verdict's tries into the derived metric `verdict.<status>.<name>`.
    pub(crate) const TIMES: [(&'static str, TimeOf); 2] =
        [("duration_ms", |t| t.duration_ms), ("cpu_ms", |t| t.cpu_ms)];
}

/// A pass/fail outcome with its tries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Verdict {
    pub status: VerdictStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tries: Vec<Try>,
}

impl Verdict {
    /// A verdict of `status` reached through `tries` (none when they were not recorded).
    #[must_use]
    pub const fn new(status: VerdictStatus, tries: Vec<Try>) -> Self {
        Self { status, tries }
    }
}

/// Why a key has no value: a reason code and what was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Reason {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Reason {
    /// The reason code `reason`: `timeout`, `build_failed`, `interrupted`.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            detail: None,
        }
    }

    /// The reason with what was observed, or with nothing for `None`.
    #[must_use]
    pub fn detail(mut self, detail: impl Optional<String>) -> Self {
        self.detail = optional(detail);
        self
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

impl Outcome {
    /// One measured value of a declared metric.
    ///
    /// # Errors
    /// [`NotFinite`] for NaN or an infinity.
    pub fn measured(metric: MetricName, value: f64) -> Result<Self, NotFinite> {
        Ok(Self::Measured(vec![Measure::new(metric, value)?]))
    }

    /// A check that passed, its tries unrecorded.
    #[must_use]
    pub const fn pass() -> Self {
        Self::Verdict(Verdict::new(VerdictStatus::Pass, Vec::new()))
    }

    /// A check that failed, its tries unrecorded.
    #[must_use]
    pub const fn fail() -> Self {
        Self::Verdict(Verdict::new(VerdictStatus::Fail, Vec::new()))
    }
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

/// The payload of `declare`: what a producer learns once the work has begun.
/// A run has at most one, before any result.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Declaration {
    /// The keys, when `open` did not name them.
    pub keys: Option<Vec<Key>>,
    /// Metrics learned late; they extend the run's declarations, never redefine one.
    pub metrics: BTreeMap<MetricName, Metric>,
    /// What was built, when it is known only now.
    pub build: Option<Build>,
}

impl Declaration {
    /// A declaration of nothing yet; add keys, metrics and a build.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The declaration naming the run's keys, when `open` did not.
    #[must_use]
    pub fn keys(mut self, keys: impl IntoIterator<Item = Key>) -> Self {
        self.keys = Some(keys.into_iter().collect());
        self
    }

    /// The declaration with one more metric.
    #[must_use]
    pub fn metric(mut self, name: MetricName, metric: Metric) -> Self {
        self.metrics.insert(name, metric);
        self
    }

    /// The declaration with what was built.
    #[must_use]
    pub fn build(mut self, build: Build) -> Self {
        self.build = Some(build);
        self
    }
}

impl From<Declaration> for Event {
    fn from(declaration: Declaration) -> Self {
        let Declaration {
            keys,
            metrics,
            build,
        } = declaration;
        Self::Declare {
            keys,
            metrics,
            build,
        }
    }
}

/// The events of the run grammar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Open(Box<Open>),
    /// The keys, the metrics and what was built, once the producer knows them.
    /// Built from a [`Declaration`].
    #[non_exhaustive]
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
    /// Built by [`Event::result`].
    #[non_exhaustive]
    Result {
        key: Key,
        outcome: Outcome,
        /// The code site that produced the value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
    },
    /// Built by [`Event::close`].
    #[non_exhaustive]
    Close {
        end: End,
        /// Readings taken while the run executed (load): provenance only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conditions: Option<serde_json::Value>,
    },
}

impl Event {
    /// A result: `key`'s `outcome`.
    #[must_use]
    pub const fn result(key: Key, outcome: Outcome) -> Self {
        Self::Result {
            key,
            outcome,
            location: None,
        }
    }

    /// A result with the code site that produced it.
    #[must_use]
    pub fn result_at(key: Key, outcome: Outcome, location: impl Into<String>) -> Self {
        Self::Result {
            key,
            outcome,
            location: Some(location.into()),
        }
    }

    /// The run's close.
    #[must_use]
    pub const fn close(end: End) -> Self {
        Self::Close {
            end,
            conditions: None,
        }
    }

    /// The run's close with the readings taken while it executed, a JSON object.
    #[must_use]
    pub const fn close_with(end: End, conditions: serde_json::Value) -> Self {
        Self::Close {
            end,
            conditions: Some(conditions),
        }
    }
}

/// One line of a note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl EventLine {
    /// The line at `seq` of `run`, written at `time`, in this crate's envelope version.
    #[must_use]
    pub fn new(run: RunId, seq: u32, time: Timestamp, event: Event) -> Self {
        Self {
            v: ENVELOPE_VERSION,
            run,
            seq,
            time: time.into(),
            event,
        }
    }
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

    fn bare() -> Open {
        Open::new(
            CommitId::parse(&"c".repeat(40)).expect("commit"),
            ProducerName::parse("bench-suite").expect("producer"),
        )
    }

    fn machine(name: &str) -> Machine {
        Machine::parse(name).expect("machine")
    }

    fn open() -> Open {
        bare()
            .version("0.41.0")
            .harness("bench-suite/7")
            .machine(machine("bench"))
            .keys([Key::parse("parse_large_d64").expect("key")])
    }

    #[test]
    fn an_open_built_from_its_commit_and_producer_alone_names_no_harness_or_machine() {
        let text = serde_json::to_string(&bare()).expect("json");
        assert_eq!(
            text,
            format!(
                r#"{{"commit":"{}","producer":{{"name":"bench-suite"}},"suite":{{"harness":"","params":{{}}}},"environment":{{"machine":"","params":{{}}}},"build":{{}}}}"#,
                "c".repeat(40)
            )
        );
    }

    #[test]
    fn builders_write_the_open_the_spec_shows() {
        let p99 = MetricName::parse("latency.p99_ns").expect("metric");
        let open = Open::new(
            CommitId::parse("9c41e2d0a1b2c3d4e5f60718293a4b5c6d7e8f90").expect("commit"),
            ProducerName::parse("bench-suite").expect("producer"),
        )
        .version("1.4.0")
        .harness("bench-suite/2")
        .param("repeats", 20)
        .metric(p99, Metric::lower("ns"))
        .machine(machine("laptop"))
        .keys(["parse_large", "lexer_tokens"].map(|k| Key::parse(k).expect("key")));
        // The same open as docs/FORMAT.md's example; the writer also emits
        // the empty `params` and `build` that the example leaves out.
        assert_eq!(
            serde_json::to_value(&open).expect("json"),
            serde_json::json!({
                "commit": "9c41e2d0a1b2c3d4e5f60718293a4b5c6d7e8f90",
                "producer": {"name": "bench-suite", "version": "1.4.0"},
                "suite": {
                    "harness": "bench-suite/2",
                    "params": {"repeats": 20},
                    "metrics": {"latency.p99_ns": {"unit": "ns", "better": "lower"}},
                },
                "environment": {"machine": "laptop", "params": {}},
                "build": {},
                "keys": ["parse_large", "lexer_tokens"],
            })
        );
        let suite = Suite::new()
            .harness("bench-suite/2")
            .param("repeats", 20)
            .metric(
                MetricName::parse("latency.p99_ns").expect("metric"),
                Metric::lower("ns"),
            );
        assert_eq!(open.suite, suite);
    }

    #[test]
    fn every_part_of_an_open_has_a_builder() {
        let environment = Environment::new()
            .machine(machine("bench"))
            .param("host_role", "dedicated")
            .descriptor("os", "Linux 6.13");
        let build = Build::new()
            .profile(
                Profile::new()
                    .target("x86_64-unknown-linux-gnu")
                    .flags("release"),
            )
            .toolchain("1.95.0")
            .artifact(Sha256::of(b"binary"));
        let open = bare()
            .environment(environment.clone())
            .build(build.clone())
            .source("ci/run/4127");
        assert_eq!(open.environment, environment);
        assert_eq!(open.environment.machine, "bench");
        assert_eq!(open.environment.params["host_role"], "dedicated");
        assert_eq!(open.environment.descriptor["os"], "Linux 6.13");
        assert_eq!(
            open.build.profile.as_ref().and_then(|p| p.flags.as_deref()),
            Some("release")
        );
        assert_eq!(open.build.toolchain.as_deref(), Some("1.95.0"));
        assert_eq!(open.build, build);
        assert_eq!(open.source.as_deref(), Some("ci/run/4127"));
        // An environment replaces the machine named before it.
        let replaced = bare()
            .machine(machine("bench"))
            .environment(Environment::new());
        assert_eq!(replaced.environment.machine, "");
        // A suite replaces the harness named before it; a harness after it stays.
        let suite = Suite::new().harness("a/1").param("repeats", 3);
        let replaced = bare().harness("b/1").suite(suite.clone());
        assert_eq!(replaced.suite, suite);
        assert_eq!(bare().suite(suite).harness("c/1").suite.harness, "c/1");
    }

    #[test]
    fn an_optional_field_takes_a_value_or_an_option() {
        let unknown: Option<f64> = None;
        let t = Try::new(VerdictStatus::Pass)
            .duration_ms(12.5)
            .cpu_ms(unknown);
        assert_eq!((t.duration_ms, t.cpu_ms), (Some(12.5), None));
        let t = t.cpu_ms(Some(30.0));
        assert_eq!(t.cpu_ms, Some(30.0));
        let m = Measure::new(MetricName::parse("a").expect("metric"), 1.0)
            .expect("finite")
            .n(20)
            .mad(310.0);
        assert_eq!((m.n, m.mad), (Some(20), Some(310.0)));
        let m = m.n(None).mad(None);
        assert_eq!((m.n, m.mad), (None, None));
        let observed: Option<String> = Some("SIGTERM".to_owned());
        assert_eq!(
            Reason::new("interrupted")
                .detail(observed)
                .detail
                .as_deref(),
            Some("SIGTERM")
        );
        assert_eq!(
            Reason::new("timeout")
                .detail("after 30 s")
                .detail
                .as_deref(),
            Some("after 30 s")
        );
        assert_eq!(
            Reason::new("timeout")
                .detail(String::from("after 30 s"))
                .detail(None::<&str>)
                .detail,
            None
        );
        let name = ProducerName::parse("bench-suite").expect("producer");
        assert_eq!(
            Producer::new(name.clone()).version(Some("1.4.0")).version,
            Some("1.4.0".to_owned())
        );
        assert_eq!(
            bare().version("1.4.0").producer,
            Producer::new(name).version("1.4.0")
        );
        assert_eq!(bare().source(None::<String>).source, None);
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
                outcome: Outcome::Refused(Reason::new("noisy_host")),
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
        let mut same = base
            .clone()
            .build(
                Build::new()
                    .artifact(Sha256::of(b"another binary"))
                    .toolchain("1.95.0"),
            )
            .environment(
                Environment::new()
                    .machine(machine("bench"))
                    .descriptor("os", "Linux 6.13"),
            );
        same.commit = CommitId::parse(&"d".repeat(40)).expect("commit");
        assert_eq!(variant_of("perf", &base), variant_of("perf", &same));
        let debug = base
            .clone()
            .build(Build::new().profile(Profile::new().flags("debug")));
        assert_ne!(variant_of("perf", &base), variant_of("perf", &debug));
        let other_machine = base.clone().machine(machine("laptop"));
        assert_ne!(
            variant_of("perf", &base),
            variant_of("perf", &other_machine)
        );
        assert_ne!(variant_of("perf", &base), variant_of("gates", &base));
    }
}
