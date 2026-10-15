use lighthouse_model::wilson;

#[test]
fn wilson_matches_the_published_example_and_bounds_the_extremes() {
    // 81 of 263: the textbook interval is about 0.2553 to 0.3662.
    let (low, high) = wilson(81.0, 263.0);
    assert!((low - 0.2553).abs() < 0.001, "{low}");
    assert!((high - 0.3662).abs() < 0.001, "{high}");

    assert_eq!(wilson(0.0, 10.0).0, 0.0);
    assert!((wilson(10.0, 10.0).1 - 1.0).abs() < 1e-12);
    // Weights need not be whole.
    let (low, high) = wilson(0.7, 1.4);
    assert!(low < 0.5 && 0.5 < high);
}
