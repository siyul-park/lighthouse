use std::fs;

use lighthouse_cache::{Cache, Key, KeyBuilder};

fn key(name: &str) -> Key {
    let mut key = KeyBuilder::new();
    key.part(name);
    key.finish()
}

#[test]
fn a_stored_value_is_found_by_a_later_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), u64::MAX).unwrap();
    cache.put(key("a"), &vec!["one".to_owned()]);
    assert_eq!(
        cache.get::<Vec<String>>(&key("a")),
        None,
        "written at the end"
    );
    assert_eq!(cache.finish().unwrap().stored, 1);

    let later = Cache::open(dir.path(), u64::MAX).unwrap();
    assert_eq!(
        later.get::<Vec<String>>(&key("a")),
        Some(vec!["one".to_owned()])
    );
    assert_eq!(later.get::<Vec<String>>(&key("b")), None);
}

#[test]
fn a_damaged_database_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let garbage = "this is not a database, but long enough to look like one".repeat(100);
    fs::write(dir.path().join("results.db"), garbage).unwrap();
    let cache = Cache::open(dir.path(), u64::MAX).unwrap();
    cache.put(key("a"), &1_u32);
    cache.finish().unwrap();
    let later = Cache::open(dir.path(), u64::MAX).unwrap();
    assert_eq!(later.get::<u32>(&key("a")), Some(1));
}

#[test]
fn a_database_of_another_schema_is_emptied() {
    let dir = tempfile::tempdir().unwrap();
    {
        let old = rusqlite::Connection::open(dir.path().join("results.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE results (key BLOB PRIMARY KEY, diagnostics BLOB, notices BLOB, used_at INTEGER);
             INSERT INTO results VALUES (x'00', x'5b5d', x'5b5d', 0);",
        )
        .unwrap();
    }
    let cache = Cache::open(dir.path(), u64::MAX).unwrap();
    cache.put(key("a"), &1_u32);
    cache.finish().unwrap();
    let later = Cache::open(dir.path(), u64::MAX).unwrap();
    assert_eq!(later.get::<u32>(&key("a")), Some(1));
}

#[test]
fn a_directory_that_cannot_be_created_is_an_error_not_a_reset() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("cache");
    fs::write(&blocked, "a file").unwrap();
    assert!(Cache::open(&blocked, u64::MAX).is_err());
    assert_eq!(fs::read_to_string(&blocked).unwrap(), "a file");
}

#[test]
fn pruning_drops_the_oldest_until_the_database_is_under_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 60_000).unwrap();
    for n in 0..10 {
        cache.put(key(&n.to_string()), &"x".repeat(10_000));
    }
    let stats = cache.finish().unwrap();
    assert!(stats.pruned > 0 && stats.pruned < 10, "{stats:?}");

    let later = Cache::open(dir.path(), u64::MAX).unwrap();
    let left = (0..10)
        .filter(|n| later.get::<String>(&key(&n.to_string())).is_some())
        .count();
    assert_eq!(left, 10 - stats.pruned);
}

#[test]
fn nothing_is_pruned_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), 1 << 20).unwrap();
    cache.put(key("a"), &"x".repeat(1000));
    assert_eq!(cache.finish().unwrap().pruned, 0);
}

#[test]
fn key_parts_are_told_apart_by_their_lengths() {
    let of = |parts: &[&str]| {
        let mut key = KeyBuilder::new();
        parts.iter().for_each(|part| {
            key.part(part);
        });
        key.finish()
    };
    assert_ne!(of(&["ab", "c"]), of(&["a", "bc"]));
    assert_ne!(of(&["a"]), of(&["a", ""]));
    assert_eq!(of(&["a", "b"]), of(&["a", "b"]));
}

#[test]
fn cleaning_deletes_the_directory_and_what_a_plugin_kept_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    Cache::open(&cache, u64::MAX).unwrap();
    fs::create_dir_all(cache.join("lang-go")).unwrap();
    fs::write(cache.join("lang-go/units.json"), "{}").unwrap();
    assert!(lighthouse_cache::clean(&cache).unwrap());
    assert!(!cache.exists());
    assert!(!lighthouse_cache::clean(&cache).unwrap());
}

#[test]
fn key_as_bytes_is_a_sha256() {
    assert_eq!(key("a").as_bytes().len(), 32);
    assert_ne!(key("a").as_bytes(), key("b").as_bytes());
}

#[test]
fn cache_put_bytes_stores_what_get_reads() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path(), u64::MAX).unwrap();
    cache.put_bytes(key("a"), b"[1,2]".to_vec());
    cache.finish().unwrap();
    let later = Cache::open(dir.path(), u64::MAX).unwrap();
    assert_eq!(later.get::<Vec<u8>>(&key("a")), Some(vec![1, 2]));
}

#[test]
fn build_identity_names_the_version_and_the_program() {
    let identity = lighthouse_cache::build_identity();
    assert!(
        identity.starts_with(env!("CARGO_PKG_VERSION")),
        "{identity}"
    );
    assert_eq!(identity, lighthouse_cache::build_identity());
}
