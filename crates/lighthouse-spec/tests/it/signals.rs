//! What a project says about session signals: how long to keep them and
//! whether the agent hears of them.

use std::time::Duration;

use lighthouse_spec::{Config, Surface};

fn config(signals: &str) -> Config {
    Config::parse_inline(&format!("plugins = [\"core\"]\n{signals}")).unwrap()
}

#[test]
fn config_signals() {
    let quiet = config("");
    assert!(quiet.signals().is_empty());

    let set = config("[signals]\nretention = \"7d\"\nsurface = \"on\"\n");
    assert!(!set.signals().is_empty());
    assert_eq!(set.signals().surface(), Surface::On);
    assert_eq!(set.signals().retention(), Duration::from_secs(7 * 86_400));

    let unknown = Config::parse_inline("plugins = [\"core\"]\n[signals]\nsurface = \"loud\"\n");
    assert!(unknown.is_err());
}

#[test]
fn signals_spec_retention() {
    assert_eq!(
        config("").signals().retention(),
        Duration::from_secs(30 * 86_400)
    );
    for (text, seconds) in [("12h", 12 * 3600), ("90d", 90 * 86_400), ("45m", 45 * 60)] {
        let set = config(&format!("[signals]\nretention = \"{text}\"\n"));
        assert_eq!(
            set.signals().retention(),
            Duration::from_secs(seconds),
            "{text}"
        );
    }
    assert!(
        Config::parse_inline("plugins = [\"core\"]\n[signals]\nretention = \"soon\"\n").is_err()
    );
}

#[test]
fn signals_spec_surface() {
    assert_eq!(config("").signals().surface(), Surface::Shadow);
    assert_eq!(
        config("[signals]\nsurface = \"shadow\"\n")
            .signals()
            .surface(),
        Surface::Shadow
    );
    assert_eq!(
        config("[signals]\nsurface = \"on\"\n").signals().surface(),
        Surface::On
    );
}
