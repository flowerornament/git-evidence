//! Validated identifiers. Each format is checked once, at construction, so the
//! rest of the crate never re-checks hex digits or name characters. Git's own
//! object ids (`ObjectId`, `CommitId`, `TreeId`) and the shared refusal
//! `IdError` are `git-process`'s, re-exported here.

use serde::{Deserialize, Serialize};
use std::fmt;

pub use git_process::{CommitId, IdError, ObjectId, TreeId};

fn is_lower_hex(text: &str) -> bool {
    text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The validated text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(value: String) -> Result<Self, IdError> {
                Self::parse(&value)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdError;
            fn from_str(text: &str) -> Result<Self, IdError> {
                Self::parse(text)
            }
        }

        /// The index stores a name as its text.
        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.0.as_str().into())
            }
        }
    };
}

string_id! {
    /// A SHA-256 digest, written `sha256:<64 hex>`.
    Sha256
}

impl Sha256 {
    /// # Errors
    /// Refuses anything but `sha256:` followed by 64 lowercase hex digits.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        match text.strip_prefix("sha256:") {
            Some(hex) if hex.len() == 64 && is_lower_hex(hex) => Ok(Self(text.to_owned())),
            _ => Err(IdError::new(
                "sha256 digest",
                text,
                "sha256: and 64 lowercase hex digits",
            )),
        }
    }

    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(bytes);
        let mut text = String::with_capacity(71);
        text.push_str("sha256:");
        for byte in digest {
            use fmt::Write as _;
            write!(text, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(text)
    }
}

fn is_name(text: &str, max: usize) -> bool {
    let mut bytes = text.bytes();
    let starts = matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'));
    starts
        && text.len() <= max
        && text
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'))
        && !text.contains("..")
        && !text.ends_with('.')
}

/// `text` as a lowercase name (`[a-z0-9][a-z0-9._-]*`, at most 64 bytes),
/// or why not, naming `what` it was to be.
fn lower_name(what: &'static str, text: &str) -> Result<String, IdError> {
    if is_name(text, 64) {
        Ok(text.to_owned())
    } else {
        Err(IdError::new(
            what,
            text,
            "lowercase [a-z0-9][a-z0-9._-]*, at most 64 bytes",
        ))
    }
}

string_id! {
    /// What a piece of evidence is: `perf`, `tests`, `checks`. The namespace
    /// readers select by, and the ref path component that names it.
    Kind
}

impl Kind {
    /// # Errors
    /// Refuses names that are not lowercase `[a-z0-9][a-z0-9._-]*`, at most 64 bytes.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        lower_name("evidence kind", text).map(Self)
    }
}

string_id! {
    /// A tool that produces evidence: `bench-suite`, `ci-checks`. Provenance carried
    /// in the run, never an address.
    ProducerName
}

impl ProducerName {
    /// # Errors
    /// Refuses names that are not lowercase `[a-z0-9][a-z0-9._-]*`, at most 64 bytes.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        lower_name("producer name", text).map(Self)
    }
}

string_id! {
    /// The machine a store's producers run on: `bench`, `laptop`. A declared
    /// name, never a hostname.
    Machine
}

impl Machine {
    /// # Errors
    /// Refuses an empty name, one over 64 bytes, and one with whitespace, `/` or
    /// a control character.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if !text.is_empty()
            && text.len() <= 64
            && text
                .chars()
                .all(|c| !c.is_whitespace() && !c.is_control() && c != '/')
        {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::new(
                "machine name",
                text,
                "1-64 bytes without whitespace, '/' or control characters",
            ))
        }
    }
}

fn is_label(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && text
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && c != '/')
}

string_id! {
    /// A key a run declares: a benchmark, a gate step, a test.
    Key
}

impl Key {
    /// A key is the producer's own name for what it answers (a nextest test id
    /// such as `crate::bin/name`, a generated case with its coordinates), so it
    /// is held as given: any text of 1–1024 bytes without control characters or
    /// surrounding whitespace. It is never a path or a ref.
    ///
    /// # Errors
    /// Refuses an empty or over-long key, control characters, and leading or
    /// trailing whitespace.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if !text.is_empty()
            && text.len() <= 1024
            && text.trim() == text
            && !text.chars().any(char::is_control)
        {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::new(
                "key",
                text,
                "1-1024 bytes, no control characters or surrounding whitespace",
            ))
        }
    }
}

string_id! {
    /// A metric of a measured key: `block.p99_ns`.
    MetricName
}

impl MetricName {
    /// # Errors
    /// Refuses empty names, names over 200 bytes, and names with whitespace, controls or `/`.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if is_label(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::new(
                "metric name",
                text,
                "1-200 bytes without whitespace, controls or '/'",
            ))
        }
    }
}

string_id! {
    /// A run's identity: a ULID, unique without coordination. Identity only: runs
    /// are ordered by their open event's time, never by id.
    RunId
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl RunId {
    /// # Errors
    /// Refuses anything but 26 Crockford base32 characters.
    pub fn parse(text: &str) -> Result<Self, IdError> {
        if text.len() == 26 && text.bytes().all(|b| CROCKFORD.contains(&b)) {
            Ok(Self(text.to_owned()))
        } else {
            Err(IdError::new(
                "run id",
                text,
                "a ULID: 26 Crockford base32 characters",
            ))
        }
    }

    /// A new run id: 48 bits of milliseconds since the Unix epoch, then 80 bits
    /// derived from the process id, a nanosecond reading and a counter.
    #[must_use]
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let seed = format!(
            "{}:{}:{}",
            std::process::id(),
            now.as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        Self::ulid(now.as_millis(), seed.as_bytes())
    }

    /// The id of an imported record: a ULID whose time is the record's own and
    /// whose remaining bits come from `source`, the bytes the producer says
    /// identify the run (for an importer, the record's identity plus
    /// the events it projected), so the same record projected the same way names
    /// the same run however many times, and through however many stores, it arrives.
    #[must_use]
    pub fn derived(recorded_millis: u128, source: &[u8]) -> Self {
        Self::ulid(recorded_millis, source)
    }

    fn ulid(millis: u128, entropy_source: &[u8]) -> Self {
        let entropy = Sha256::of(entropy_source);
        let hex = &entropy.as_str()["sha256:".len()..];
        let random = u128::from_str_radix(&hex[..20], 16).unwrap_or(0);
        let value = ((millis & ((1 << 48) - 1)) << 80) | (random & ((1 << 80) - 1));
        let mut text = String::with_capacity(26);
        for index in 0..26 {
            let digit = usize::try_from((value >> (5 * (25 - index))) & 0x1f).unwrap_or(0);
            text.push(char::from(CROCKFORD[digit]));
        }
        Self(text)
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_round_trip_and_hash_bytes() {
        let digest = Sha256::of(b"abc");
        assert_eq!(
            digest.as_str(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Sha256::parse(digest.as_str()), Ok(digest));
        assert!(Sha256::parse("ba7816bf").is_err());
    }

    #[test]
    fn names_cannot_escape_a_ref_namespace() {
        assert!(Kind::parse("perf").is_ok());
        assert!(Kind::parse("gates").is_ok());
        for bad in [
            "",
            "Perf",
            "-x",
            "a/b",
            "a..b",
            "a.",
            "a b",
            &"a".repeat(65),
        ] {
            assert!(Kind::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn run_ids_are_unique_ulids_and_imports_derive_theirs() {
        let first = RunId::new();
        let second = RunId::new();
        assert_ne!(first, second);
        assert_eq!(RunId::parse(first.as_str()), Ok(first.clone()));
        assert!(RunId::parse("bench/01JA8Z3K4M0000000000000000").is_err());
        let derived = RunId::derived(1_776_420_996_000, b"row");
        assert_eq!(derived, RunId::derived(1_776_420_996_000, b"row"));
        assert!(
            derived < RunId::derived(1_776_420_997_000, b"row"),
            "time comes first"
        );
    }

    #[test]
    fn identifiers_serialize_as_their_text_and_refuse_bad_input() {
        let key = Key::parse("parse_large_d64").expect("key");
        assert_eq!(
            serde_json::to_string(&key).expect("json"),
            "\"parse_large_d64\""
        );
        assert!(serde_json::from_str::<Key>("\" padded\"").is_err());
        assert!(serde_json::from_str::<Key>("\"a\\nb\"").is_err());
        for test_id in [
            "gate-format-probe::bin/gate-format-probe",
            "parser::tests::parse_large",
            &"x".repeat(1024),
        ] {
            assert!(
                Key::parse(test_id).is_ok(),
                "{test_id:?}: a producer's own id is held as given"
            );
        }
        assert!(Key::parse(&"x".repeat(1025)).is_err());
        let commit: CommitId =
            serde_json::from_str(&format!("\"{}\"", "b".repeat(40))).expect("commit");
        assert_eq!(commit.as_str(), "b".repeat(40));
    }
}
