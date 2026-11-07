use std::fs;

use lighthouse_engine::cache::{Cache, Digest, KeyBuilder};

fn key(name: &str) -> Digest {
    let mut key = KeyBuilder::new();
    key.part(name);
    key.finish()
}

#[test]
fn a_stored_value_is_found_by_a_later_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.put(key("a"), &vec!["one".to_owned()]);
    assert_eq!(
        cache.get::<Vec<String>>(&key("a")),
        None,
        "written at the end"
    );
    assert_eq!(cache.finish().unwrap().stored, 1);

    let later = Cache::open(dir.path()).unwrap();
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
    let cache = Cache::open(dir.path()).unwrap();
    cache.put(key("a"), &1_u32);
    cache.finish().unwrap();
    let later = Cache::open(dir.path()).unwrap();
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
    let cache = Cache::open(dir.path()).unwrap();
    cache.put(key("a"), &1_u32);
    cache.finish().unwrap();
    let later = Cache::open(dir.path()).unwrap();
    assert_eq!(later.get::<u32>(&key("a")), Some(1));
}

#[test]
fn a_directory_that_cannot_be_created_is_an_error_not_a_reset() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("cache");
    fs::write(&blocked, "a file").unwrap();
    assert!(Cache::open(&blocked).is_err());
    assert_eq!(fs::read_to_string(&blocked).unwrap(), "a file");
}

#[test]
fn trimming_drops_the_oldest_and_the_file_shrinks() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    for n in 0..10 {
        cache.put(key(&n.to_string()), &"x".repeat(10_000));
    }
    cache.finish().unwrap();
    cache.trim(0).unwrap();
    let file = dir.path().join("results.db");
    let full = fs::metadata(&file).unwrap().len();

    cache.trim(50_000).unwrap();

    let later = Cache::open(dir.path()).unwrap();
    let left = (0..10)
        .filter(|n| later.get::<String>(&key(&n.to_string())).is_some())
        .count();
    assert!((4..=5).contains(&left), "{left} left");
    assert!(fs::metadata(&file).unwrap().len() < full * 3 / 4);
}

#[test]
fn trimming_nothing_returns_only_the_free_pages() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.put(key("a"), &"x".repeat(100_000));
    cache.finish().unwrap();
    cache.trim(u64::MAX).unwrap();
    cache.put(key("b"), &1_u32);
    cache.finish().unwrap();

    cache.trim(0).unwrap();

    let later = Cache::open(dir.path()).unwrap();
    assert_eq!(later.get::<u32>(&key("b")), Some(1));
    assert!(fs::metadata(dir.path().join("results.db")).unwrap().len() < 50_000);
}

#[test]
fn a_database_made_without_incremental_vacuum_takes_it_and_keeps_its_results() {
    let dir = tempfile::tempdir().unwrap();
    {
        let old = rusqlite::Connection::open(dir.path().join("results.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE results (key BLOB PRIMARY KEY, value BLOB NOT NULL, used_at INTEGER NOT NULL) WITHOUT ROWID;
             CREATE INDEX results_used ON results (used_at);
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }
    drop(Cache::open(dir.path()).unwrap());
    let db = rusqlite::Connection::open(dir.path().join("results.db")).unwrap();
    let mode: i64 = db
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, 2);
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
    Cache::open(&cache).unwrap();
    fs::create_dir_all(cache.join("lang-go")).unwrap();
    fs::write(cache.join("lang-go/units.json"), "{}").unwrap();
    assert!(lighthouse_engine::cache::clean(&cache).unwrap());
    assert!(!cache.exists());
    assert!(!lighthouse_engine::cache::clean(&cache).unwrap());
}

#[test]
fn cache_put_bytes_stores_what_get_reads() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(dir.path()).unwrap();
    cache.put_bytes(key("a"), b"[1,2]".to_vec());
    cache.finish().unwrap();
    let later = Cache::open(dir.path()).unwrap();
    assert_eq!(later.get::<Vec<u8>>(&key("a")), Some(vec![1, 2]));
}

#[test]
fn build_identity_names_the_version_and_the_program() {
    let identity = lighthouse_engine::cache::build_identity();
    assert!(
        identity.starts_with(env!("CARGO_PKG_VERSION")),
        "{identity}"
    );
    assert_eq!(identity, lighthouse_engine::cache::build_identity());
}

#[test]
fn dir_is_the_cache_folder_under_the_state_directory() {
    assert_eq!(
        lighthouse_engine::cache::dir(std::path::Path::new("/p")),
        std::path::Path::new("/p/.lighthouse/cache")
    );
}
