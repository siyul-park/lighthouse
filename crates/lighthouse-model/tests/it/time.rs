use std::time;

use lighthouse_model::Duration;

#[test]
fn now_is_rfc_3339_in_utc_with_milliseconds() {
    let now = lighthouse_model::now();

    let (date, time) = now.split_once('T').unwrap();
    assert_eq!(date.len(), 10);
    assert!(date.starts_with("20"), "{now}");
    assert_eq!(time.len(), 13, "{now}");
    assert!(time.ends_with('Z'), "{now}");
    assert_eq!(&time[2..3], ":");
    assert_eq!(&time[8..9], ".");
}

#[test]
fn durations_need_a_unit_and_a_span() {
    let parse = |text: &str| text.parse::<Duration>().ok().map(Duration::get);
    assert_eq!(parse("30s"), Some(time::Duration::from_secs(30)));
    assert_eq!(parse("2m"), Some(time::Duration::from_secs(120)));
    assert_eq!(parse("500ms"), Some(time::Duration::from_millis(500)));
    assert_eq!(parse("1h"), Some(time::Duration::from_secs(3600)));
    assert_eq!(parse("30"), None);
    assert_eq!(parse("s"), None);
    assert_eq!(parse("3d"), Some(time::Duration::from_secs(259_200)));
    assert_eq!(parse("3w"), None);
    assert_eq!(parse("0s"), None);
    assert_eq!(Duration::from_secs(90).get(), time::Duration::from_secs(90));
}

#[test]
fn a_duration_is_written_in_the_largest_exact_unit_and_read_back() {
    for (text, written) in [
        ("90s", "90s"),
        ("60s", "1m"),
        ("7200s", "2h"),
        ("172800s", "2d"),
        ("1500ms", "1500ms"),
    ] {
        let duration: Duration = text.parse().unwrap();
        let json = serde_json::to_value(duration).unwrap();
        assert_eq!(json, written);
        assert_eq!(serde_json::from_value::<Duration>(json).unwrap(), duration);
    }
    let error = serde_json::from_value::<Duration>("soon".into()).unwrap_err();
    assert!(error.to_string().contains("not a duration"), "{error}");
}
