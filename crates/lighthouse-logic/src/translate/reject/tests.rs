use super::{Construct, Reject};

#[test]
fn a_construct_is_named_by_a_token_and_told_in_prose() {
    let ternary = Construct::Ternary;
    let call = Construct::Call("regex.matches".to_owned());

    assert_eq!(ternary.as_str(), "ternary");
    assert_eq!(call.as_str(), "regex.matches");
    assert_eq!(call.to_string(), "the call `regex.matches`");
}

#[test]
fn several_prose_of_one_construct_share_its_token() {
    assert_eq!(Construct::Expansion.as_str(), Construct::Cases.as_str());
    assert_eq!(Construct::Arity("f".to_owned()).as_str(), "arguments");
    assert_eq!(Construct::Bare("f".to_owned()).as_str(), "arguments");
}

#[test]
fn a_reject_gives_the_token_of_its_construct_and_says_where_it_is_outside() {
    let reject = Reject::from(Construct::TwoFields);

    assert_eq!(reject.construct(), "two-fields");
    assert_eq!(
        reject.to_string(),
        "outside the fragment: a comparison of two fields"
    );
}
