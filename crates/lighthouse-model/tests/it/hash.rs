use lighthouse_model::hash::{Hasher, sha256, short};

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
    assert_eq!(hasher.finish(), ABC);
}

#[test]
fn hasher_finish_bytes_is_the_raw_digest() {
    let mut hasher = Hasher::new();
    hasher.update("abc");
    let raw = hasher.finish_bytes();
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, ABC);
}
