//! The checks each line passes on its own, before any run or tree is read:
//! the envelope version, the one time form, finite numbers and no negative
//! time.

use super::Refusal;
use crate::format::event::{ENVELOPE_VERSION, Event, EventLine, Outcome, Timestamp, Try};

/// Each line on its own: this envelope version, the one time form, finite
/// numbers and no negative time.
pub(super) fn check_lines(lines: &[EventLine]) -> Result<(), Refusal> {
    for line in lines {
        if line.v != ENVELOPE_VERSION {
            return Err(Refusal::Envelope {
                run: line.run.clone(),
                found: line.v,
            });
        }
        if Timestamp::parse(&line.time).is_err() {
            return Err(Refusal::Time {
                run: line.run.clone(),
                seq: line.seq,
                time: line.time.clone(),
            });
        }
        if let Some(field) = non_finite(&line.event) {
            return Err(Refusal::NonFinite {
                run: line.run.clone(),
                seq: line.seq,
                field,
            });
        }
        if let Some(field) = negative(&line.event) {
            return Err(Refusal::Negative {
                run: line.run.clone(),
                seq: line.seq,
                field,
            });
        }
    }
    Ok(())
}

/// The first number in an event that is NaN or infinite. JSON cannot hold one
/// (serde writes `null`), so recording it would silently say something other
/// than what was observed.
fn non_finite(event: &Event) -> Option<String> {
    let Event::Result { key, outcome, .. } = event else {
        return None;
    };
    match outcome {
        Outcome::Measured(measures) => measures.iter().find_map(|m| {
            if !m.value.is_finite() {
                Some(format!("{key} {} value", m.metric))
            } else if m.mad.is_some_and(|mad| !mad.is_finite()) {
                Some(format!("{key} {} mad", m.metric))
            } else {
                None
            }
        }),
        Outcome::Verdict(verdict) => verdict.tries.iter().enumerate().find_map(|(n, t)| {
            Try::TIMES.iter().find_map(|(name, time)| {
                time(t)
                    .is_some_and(|d| !d.is_finite())
                    .then(|| format!("{key} try {n} {name}"))
            })
        }),
        Outcome::Refused(_) | Outcome::Skipped(_) => None,
    }
}

/// The first time in an event that is negative: a try's wall and CPU times
/// count time that passed or a process ran, so a negative one says nothing
/// that was observed.
fn negative(event: &Event) -> Option<String> {
    let Event::Result {
        key,
        outcome: Outcome::Verdict(verdict),
        ..
    } = event
    else {
        return None;
    };
    verdict.tries.iter().enumerate().find_map(|(n, t)| {
        Try::TIMES.iter().find_map(|(name, time)| {
            time(t)
                .is_some_and(|d| d < 0.0)
                .then(|| format!("{key} try {n} {name}"))
        })
    })
}
