//! Time: the current instant as text, and a span written as a number and a
//! unit.

use std::{
    borrow::Cow,
    fmt,
    str::FromStr,
    time::{self, SystemTime, UNIX_EPOCH},
};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A span of time written as a number and a unit: `500ms`, `30s`, `2m`, `1h`, `30d`.
/// A bare number has no unit and a zero span is no limit at all: both are
/// refused where the document is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Duration(time::Duration);

/// The text is not a duration such as `30s` or `2m`.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("`{0}` is not a duration such as `30s` or `2m`")]
pub struct InvalidDuration(String);

impl Duration {
    /// `secs` seconds, for a limit written in code; there is no zero span.
    pub const fn from_secs(secs: u64) -> Self {
        assert!(secs > 0, "a duration is not zero");
        Self(time::Duration::from_secs(secs))
    }

    /// The span as the standard library counts it.
    pub fn get(self) -> time::Duration {
        self.0
    }
}

impl FromStr for Duration {
    type Err = InvalidDuration;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidDuration(text.to_owned());
        let trimmed = text.trim();
        let split = trimmed
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(invalid)?;
        let n: u64 = trimmed[..split].parse().map_err(|_| invalid())?;
        let span = match &trimmed[split..] {
            "ms" => Some(time::Duration::from_millis(n)),
            "s" => Some(time::Duration::from_secs(n)),
            "m" => n.checked_mul(60).map(time::Duration::from_secs),
            "h" => n.checked_mul(3600).map(time::Duration::from_secs),
            "d" => n.checked_mul(86_400).map(time::Duration::from_secs),
            _ => None,
        };
        span.filter(|span| !span.is_zero())
            .map(Self)
            .ok_or_else(invalid)
    }
}

impl fmt::Display for Duration {
    /// In the largest unit that counts it exactly.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let millis = self.0.as_millis();
        match millis {
            m if m % 86_400_000 == 0 => write!(f, "{}d", m / 86_400_000),
            m if m % 3_600_000 == 0 => write!(f, "{}h", m / 3_600_000),
            m if m % 60_000 == 0 => write!(f, "{}m", m / 60_000),
            m if m % 1000 == 0 => write!(f, "{}s", m / 1000),
            m => write!(f, "{m}ms"),
        }
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// A string in the document.
impl JsonSchema for Duration {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Duration")
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
    }
}

impl From<Duration> for time::Duration {
    fn from(duration: Duration) -> Self {
        duration.0
    }
}

/// The current UTC time as RFC 3339 with milliseconds, such as
/// `2026-10-16T09:30:00.123Z`: the form the log and the cache record.
pub fn now() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (days, secs) = (since.as_secs() / 86_400, since.as_secs() % 86_400);
    // The civil date of a day count since 1970-01-01 (Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let (month, year) = if month_index < 10 {
        (month_index + 3, year_of_era + era * 400)
    } else {
        (month_index - 9, year_of_era + era * 400 + 1)
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60,
        since.subsec_millis()
    )
}
