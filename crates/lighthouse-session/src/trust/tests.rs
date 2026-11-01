use super::*;

#[test]
fn old_layout_reads_as_nothing_trusted() {
    let old = "[trusted]\n\"/a/b\" = \"abc\"\n";
    assert!(parse(old).unwrap().is_empty());
}

#[test]
fn roots_under_a_key_are_read() {
    let now = "[trusted.\"git:/r/.git\"]\n\"/r\" = \"abc\"\n\"/w\" = \"abc\"\n";
    let known = parse(now).unwrap();
    assert_eq!(known["git:/r/.git"].len(), 2);
}

#[test]
fn a_path_in_the_root_is_hashed_without_the_root() {
    let prefixes = prefixes(Path::new("/nonexistent/one"));
    assert_eq!(relative(&prefixes, "/nonexistent/one/x.sh"), "{root}/x.sh");
    assert_eq!(relative(&prefixes, "/usr/bin/sh"), "/usr/bin/sh");
}
