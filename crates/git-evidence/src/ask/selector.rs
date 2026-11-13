//! Which landings a question reads: a range `A..B`, a committer-date window
//! and the newest `N`, accepted by every question that spans landings
//! (`trend`, `log`). In SQL the same selection is a filter of the `landings`
//! view by `landing` and `date`.
//!
//! ```text
//!   trunk  #0 ── #1 ── #2 ── #3 ── #4 ── #5
//!   #1..#4        └──────────────┘            after A, through B
//!   --since/--until                            committer date inside the window
//!   --last N                                   the newest N of what remains
//! ```

use super::NotRecorded;
use super::landings::LandingNumber;
use crate::format::id::CommitId;
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

/// Which landings a question reads. Each part narrows the trunk: the range,
/// then the date window, then the newest `last` of what remains. Nothing set
/// selects every landing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    /// `A..B`.
    pub range: Option<LandingRange>,
    /// Landings committed at or after the start of this date.
    pub since: Option<UtcDate>,
    /// Landings committed before the end of this date.
    pub until: Option<UtcDate>,
    /// Only the newest this many.
    pub last: Option<NonZeroU32>,
}

impl Selector {
    /// Whether nothing narrows the trunk.
    #[must_use]
    pub fn is_everything(&self) -> bool {
        *self == Self::default()
    }

    /// The newest `n` landings.
    #[must_use]
    pub fn newest(n: NonZeroU32) -> Self {
        Self {
            last: Some(n),
            ..Self::default()
        }
    }
}

/// `A..B` on the trunk: the landings after commit `after`, up to and
/// including commit `through`. Both must be landings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingRange {
    pub after: CommitId,
    pub through: CommitId,
}

/// A UTC day (`YYYY-MM-DD`) or second (`YYYY-MM-DDTHH:MM:SSZ`), the span of
/// time its spelling names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcDate {
    /// Unix seconds at its start.
    start: i64,
    /// Unix seconds just after its end.
    end: i64,
}

/// Why a date was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    /// Not spelled `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`.
    #[error("{0:?} is not a UTC date: write YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ")]
    Spelling(String),
    /// Spelled as a date, but no such day exists: month 13, February 30.
    #[error("{0:?} names no day of the calendar")]
    NoSuchDay(String),
    /// Spelled as a time, but no such time of day exists: 24:00:00.
    #[error("{0:?} names no time of day: hours 00-23, minutes and seconds 00-59")]
    NoSuchTime(String),
}

impl UtcDate {
    /// Unix seconds at its start.
    #[must_use]
    pub const fn start(self) -> i64 {
        self.start
    }

    /// Unix seconds just after its end: a day ends at the next midnight.
    #[must_use]
    pub const fn end(self) -> i64 {
        self.end
    }
}

impl FromStr for UtcDate {
    type Err = DateError;

    fn from_str(text: &str) -> Result<Self, DateError> {
        let refuse = || DateError::Spelling(text.to_owned());
        let number = |part: Option<&str>, digits: usize| -> Result<i64, DateError> {
            match part {
                Some(p) if p.len() == digits && p.bytes().all(|b| b.is_ascii_digit()) => {
                    p.parse().map_err(|_| refuse())
                }
                _ => Err(refuse()),
            }
        };
        let (day, time) = match text.split_once('T') {
            Some((day, time)) => (day, Some(time.strip_suffix('Z').ok_or_else(refuse)?)),
            None => (text, None),
        };
        let mut parts = day.split('-');
        let (year, month, mday) = (
            number(parts.next(), 4)?,
            number(parts.next(), 2)?,
            number(parts.next(), 2)?,
        );
        if parts.next().is_some() {
            return Err(refuse());
        }
        if !(1..=12).contains(&month) {
            return Err(DateError::NoSuchDay(text.to_owned()));
        }
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_month = match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if !(1..=days_in_month).contains(&mday) {
            return Err(DateError::NoSuchDay(text.to_owned()));
        }
        let midnight = days_from_civil(year, month, mday) * 86_400;
        let Some(time) = time else {
            return Ok(Self {
                start: midnight,
                end: midnight + 86_400,
            });
        };
        let mut parts = time.split(':');
        let (h, m, s) = (
            number(parts.next(), 2)?,
            number(parts.next(), 2)?,
            number(parts.next(), 2)?,
        );
        if parts.next().is_some() {
            return Err(refuse());
        }
        if h > 23 || m > 59 || s > 59 {
            return Err(DateError::NoSuchTime(text.to_owned()));
        }
        let start = midnight + h * 3600 + m * 60 + s;
        Ok(Self {
            start,
            end: start + 1,
        })
    }
}

impl fmt::Display for UtcDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = civil_from_days(self.start.div_euclid(86_400));
        let second = self.start.rem_euclid(86_400);
        if self.end - self.start == 86_400 && second == 0 {
            write!(f, "{year:04}-{month:02}-{day:02}")
        } else {
            write!(
                f,
                "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
                second / 3600,
                second / 60 % 60,
                second % 60
            )
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`].
const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// The landings a selector picks, oldest first; none when nothing on the
/// trunk is in the selection.
///
/// # Errors
/// [`NotRecorded::NotALanding`] for a range end off the trunk.
pub(crate) fn landings(
    db: &Connection,
    selector: &Selector,
) -> Result<Vec<LandingNumber>, crate::Error> {
    let position = |commit: &CommitId| -> Result<LandingNumber, crate::Error> {
        db.query_row(
            "SELECT landing FROM trunk WHERE commit_id = ?1",
            [commit.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| NotRecorded::NotALanding(commit.clone()).into())
    };
    let (after, through) = match &selector.range {
        None => (None, None),
        Some(range) => (
            Some(position(&range.after)?),
            Some(position(&range.through)?),
        ),
    };
    let positions: Vec<LandingNumber> = db
        .prepare(
            "SELECT landing FROM (SELECT landing FROM trunk
               WHERE (?1 IS NULL OR landing > ?1) AND (?2 IS NULL OR landing <= ?2)
                 AND (?3 IS NULL OR committed >= ?3) AND (?4 IS NULL OR committed < ?4)
               ORDER BY landing DESC LIMIT ?5)
             ORDER BY landing",
        )?
        .query_map(
            params![
                after,
                through,
                selector.since.map(UtcDate::start),
                selector.until.map(UtcDate::end),
                // SQLite reads a negative LIMIT as no limit.
                selector.last.map_or(-1, |n| i64::from(n.get()))
            ],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?;
    Ok(positions)
}

/// Positions as the JSON array the queries read with `json_each`.
pub(crate) fn json(positions: &[LandingNumber]) -> String {
    serde_json::to_string(positions).expect("integers serialize")
}

#[cfg(test)]
mod tests {
    use super::{DateError, UtcDate};

    #[test]
    fn dates_name_a_day_or_a_second() {
        let day: UtcDate = "2026-10-01".parse().expect("day");
        assert_eq!(day.start(), 1_790_812_800);
        assert_eq!(day.end() - day.start(), 86_400);
        assert_eq!(day.to_string(), "2026-10-01");
        let second: UtcDate = "2026-10-01T12:00:00Z".parse().expect("second");
        assert_eq!(second.start(), 1_790_856_000);
        assert_eq!(second.end() - second.start(), 1);
        assert_eq!(second.to_string(), "2026-10-01T12:00:00Z");
        assert_eq!(
            "2024-02-29".parse::<UtcDate>().map(|d| d.to_string()),
            Ok("2024-02-29".to_owned())
        );
        let spelling = |t: &str| DateError::Spelling(t.to_owned());
        let day = |t: &str| DateError::NoSuchDay(t.to_owned());
        let time = |t: &str| DateError::NoSuchTime(t.to_owned());
        for (bad, why) in [
            ("2026-02-29", day("2026-02-29")),
            ("2026-13-01", day("2026-13-01")),
            ("26-10-01", spelling("26-10-01")),
            ("2026-10-01T12:00", spelling("2026-10-01T12:00")),
            ("yesterday", spelling("yesterday")),
            ("2026-10-01T24:00:00Z", time("2026-10-01T24:00:00Z")),
        ] {
            assert_eq!(bad.parse::<UtcDate>(), Err(why), "{bad}");
        }
    }
}
