use lighthouse_session::signal::scrub;

/// Fake keys, built so that no scanner takes this file for a leak.
fn fakes() -> Vec<String> {
    vec![
        format!("AKIA{}", "ABCDEFGHIJKLMNOP"),
        format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8"),
        format!("xoxb-{}", "123456789012-abcdefghijkl"),
        format!("sk_live_{}", "abcdefghijklmnop1234"),
        format!("AIza{}", "SyA1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q"),
        format!("sk-ant-{}", "api03-abcdefghijklmnopqrstuvwxyz"),
        format!(
            "eyJ{}.eyJ{}.{}",
            "hbGciOiJIUzI1", "zdWIiOiIxMjM0NTY3", "SflKxwRJSMeKKF2QT4"
        ),
        format!(
            "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
            "MIIBOgIBAAJBAKj34GkxFhD90vcNLYLInFEX6Ppy1tPf9Cnzj4p4WGeKLs1Pt8Qu"
        ),
    ]
}

#[test]
fn a_secret_of_a_known_provider_is_hidden() {
    for key in fakes() {
        let out = scrub(&format!("use {key} for the call"));
        assert!(!out.contains(&key), "{out}");
        assert!(out.contains("[REDACTED]"), "{out}");
    }
}

#[test]
fn credentials_in_urls_headers_and_assignments_are_hidden() {
    let cases = [
        ("postgres://admin:hunter2hunter2@db/prod", "hunter2hunter2"),
        (
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345",
            "abcdefghijklmnopqrstuvwxyz012345",
        ),
        (
            "password = \"correct-horse-battery\"",
            "correct-horse-battery",
        ),
        ("API_KEY=0123456789abcdef", "0123456789abcdef"),
    ];
    for (text, secret) in cases {
        let out = scrub(text);
        assert!(!out.contains(secret), "{out}");
    }
}

#[test]
fn ordinary_text_is_untouched() {
    let text = "Always use provider.Provider; the token count is low. 항상 Result를 반환해.";
    assert_eq!(scrub(text), text);
}
