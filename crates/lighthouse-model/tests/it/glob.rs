//! Module path globs: `*` stays in a segment, `**` crosses segments.

use lighthouse_model::glob::matches;

#[test]
fn a_star_stays_inside_one_segment() {
    assert!(matches("app/http", "app/*"));
    assert!(!matches("app/http/x", "app/*"));
    assert!(matches("app/http_x", "app/http*"));
    assert!(!matches("app/http", "app/*x"));
}

#[test]
fn a_double_star_crosses_segments_and_may_cross_none() {
    assert!(matches("app", "app/**"));
    assert!(matches("app/http/x", "app/**"));
    assert!(matches("app/http/x", "**/x"));
    assert!(!matches("lib/http", "app/**"));
}

#[test]
fn rust_module_separators_read_as_slashes() {
    assert!(matches("app::http", "app/*"));
    assert!(matches("app/http", "app::*"));
}
