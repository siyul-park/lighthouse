use super::*;

#[test]
fn a_replaced_variable_wins_once_and_the_rest_pass_through() {
    assert!(replace_vars([
        ("LIGHTHOUSE_REPLACED".to_owned(), Some("here".into())),
        ("PATH".to_owned(), None),
    ]));
    assert!(!replace_vars([]));
    assert_eq!(var("LIGHTHOUSE_REPLACED").as_deref(), Some("here"));
    assert_eq!(var_os("PATH"), None);
    assert_eq!(var_os("HOME"), env::var_os("HOME"));
}
