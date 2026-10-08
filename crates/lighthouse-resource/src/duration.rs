use std::time::Duration;

/// A duration written as a number and a unit: `500ms`, `30s`, `2m`, `1h`.
/// A bare number has no unit and is refused.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let n: u64 = text[..split].parse().ok()?;
    match &text[split..] {
        "ms" => Some(Duration::from_millis(n)),
        "s" => Some(Duration::from_secs(n)),
        "m" => n.checked_mul(60).map(Duration::from_secs),
        "h" => n.checked_mul(3600).map(Duration::from_secs),
        _ => None,
    }
}
