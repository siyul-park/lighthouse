use super::of;
use crate::project::SourceFile;

fn api(text: &str) -> String {
    let file = SourceFile {
        rel: "a.rs".to_owned(),
        text: text.to_owned(),
        ast: syn::parse_file(text).unwrap(),
        line_starts: Vec::new(),
    };
    of(&[&file])
}

const BASE: &str = "pub struct S { pub a: u8 }\npub fn f(x: u8) -> u8 {\n    x + 1\n}\n";

#[test]
fn a_body_edit_leaves_the_api() {
    assert_eq!(api(BASE), api(&BASE.replace("x + 1", "x + 2")));
}

#[test]
fn comments_and_whitespace_leave_the_api() {
    let edited = "// about\npub struct S {   pub a: u8 }  /* c */\n\n\npub fn f(x: u8) -> u8 { x + 1 } // done\n";
    assert_eq!(api(BASE), api(edited));
}

#[test]
fn doc_comments_leave_the_api_because_the_fragment_carries_them() {
    let documented =
        format!("//! The file.\n/// The type.\n{BASE}").replace("pub a", "/// A field.\n    pub a");
    assert_eq!(api(BASE), api(&documented));
}

#[test]
fn a_signature_a_field_or_a_name_moves_the_api() {
    for edited in [
        BASE.replace("x: u8", "x: u16"),
        BASE.replace("pub a: u8", "pub a: u16"),
        BASE.replace("fn f", "fn g"),
        BASE.replace("pub fn", "fn"),
    ] {
        assert_ne!(api(BASE), api(&edited), "{edited}");
    }
}
