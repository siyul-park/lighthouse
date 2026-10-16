use std::time::{SystemTime, UNIX_EPOCH};

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
