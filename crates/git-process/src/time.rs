//! A moment in UTC, to the millisecond, spelled `YYYY-MM-DDTHH:MM:SS.mmmZ`:
//! fixed width, so times sort as text. Failed-start records carry it, and so
//! does any caller's record that must sort beside them.

use std::fmt;

/// A UTC time to the millisecond, held in its one wire spelling
/// `YYYY-MM-DDTHH:MM:SS.mmmZ`. Only a well-formed spelling becomes one, so a
/// holder never checks it again; ordering is the text's, which is the time's.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct Timestamp(String);

/// A spelling that is not `YYYY-MM-DDTHH:MM:SS.mmmZ`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not YYYY-MM-DDTHH:MM:SS.mmmZ")]
pub struct NotATimestamp(pub String);

impl Timestamp {
    /// `text` as a time.
    ///
    /// # Errors
    /// [`NotATimestamp`] unless `text` is `YYYY-MM-DDTHH:MM:SS.mmmZ` with
    /// fields in range.
    pub fn parse(text: &str) -> Result<Self, NotATimestamp> {
        Self::try_from(text.to_owned())
    }

    /// The time `millis` milliseconds after the Unix epoch.
    #[must_use]
    pub fn from_unix_millis(millis: u128) -> Self {
        let seconds = u64::try_from(millis / 1000).unwrap_or(u64::MAX);
        let ms = millis % 1000;
        let days = i64::try_from(seconds / 86_400).unwrap_or(0);
        let secs = seconds % 86_400;
        // Howard Hinnant's days-to-civil algorithm.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        Self(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60
        ))
    }

    /// The current time.
    #[must_use]
    pub fn now() -> Self {
        Self::from_unix_millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis()),
        )
    }

    /// The wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Timestamp {
    type Error = NotATimestamp;

    fn try_from(text: String) -> Result<Self, NotATimestamp> {
        if well_formed(&text) {
            Ok(Self(text))
        } else {
            Err(NotATimestamp(text))
        }
    }
}

impl From<Timestamp> for String {
    fn from(time: Timestamp) -> Self {
        time.0
    }
}

/// Whether `time` is `YYYY-MM-DDTHH:MM:SS.mmmZ` with fields in range.
fn well_formed(time: &str) -> bool {
    let bytes = time.as_bytes();
    let shaped = bytes.len() == 24
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            19 => *b == b'.',
            23 => *b == b'Z',
            _ => b.is_ascii_digit(),
        });
    let field = |at: usize| time[at..at + 2].parse::<u32>().unwrap_or(99);
    shaped
        && (1..=12).contains(&field(5))
        && (1..=31).contains(&field(8))
        && field(11) <= 23
        && field(14) <= 59
        && field(17) <= 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_have_one_sortable_form() {
        let time = Timestamp::from_unix_millis(1_776_420_996_698);
        assert_eq!(time.as_str(), "2026-04-17T10:16:36.698Z");
        assert_eq!(Timestamp::parse(time.as_str()), Ok(time));
        assert!(Timestamp::parse(Timestamp::now().as_str()).is_ok());
        assert!(Timestamp::parse("2026-04-17T10:16:36Z").is_err());
        assert!(Timestamp::parse("2026-04-17T10:16:36.5Z").is_err());
        assert!(Timestamp::parse("2026-13-17T10:16:36.500Z").is_err());
    }

    #[test]
    fn only_the_one_spelling_deserializes() {
        let time: Timestamp = serde_json::from_str("\"2026-04-17T10:16:36.698Z\"").expect("a time");
        assert_eq!(
            serde_json::to_string(&time).expect("JSON"),
            "\"2026-04-17T10:16:36.698Z\""
        );
        assert!(serde_json::from_str::<Timestamp>("\"2026-04-17\"").is_err());
    }
}
