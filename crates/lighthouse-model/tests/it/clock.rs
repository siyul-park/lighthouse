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
