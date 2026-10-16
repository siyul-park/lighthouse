use lighthouse_model::hash::{Hasher, digest, sha256, short, short_digest};

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn sha256_is_the_hex_digest() {
    assert_eq!(sha256("abc"), ABC);
    assert_eq!(sha256(b"abc"), ABC);
}

#[test]
fn short_keeps_the_first_bytes_of_the_digest() {
    assert_eq!(short("abc", 4), &ABC[..8]);
    assert_eq!(short("abc", 32), ABC);
}

#[test]
fn hasher_digests_what_it_was_fed_in_pieces() {
    let mut hasher = Hasher::new();
    hasher.update("a");
    hasher.update(b"bc");
    assert_eq!(hasher.hex(), ABC);
}

#[test]
fn hasher_finish_is_the_raw_digest() {
    let mut hasher = Hasher::new();
    hasher.update("abc");
    let raw = hasher.finish();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, ABC);
}

#[test]
fn a_json_value_is_digested_by_its_canonical_form() {
    let value = serde_json::json!({ "b": 1, "a": [true, null] });

    assert_eq!(digest(&value), sha256(r#"{"a":[true,null],"b":1}"#));
    assert_eq!(short_digest(&value, 4), digest(&value)[..8]);
}
