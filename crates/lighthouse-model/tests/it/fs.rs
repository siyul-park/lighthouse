use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use lighthouse_model::{
    Base, file_in, read_optional, slashed, write_atomic, write_atomic_guarded, xdg_dir, xdg_dir_in,
    xdg_plugins,
};

#[test]
fn file_in_is_the_first_name_that_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(file_in(dir.path(), &["a.toml", "b.toml"]), None);

    std::fs::create_dir(dir.path().join("a.toml")).unwrap();
    std::fs::write(dir.path().join("b.toml"), "").unwrap();
    std::fs::write(dir.path().join("c.toml"), "").unwrap();

    assert_eq!(
        file_in(dir.path(), &["a.toml", "c.toml", "b.toml"]),
        Some(dir.path().join("c.toml")),
        "a directory is not a file, and the order of the names decides"
    );
}

#[test]
fn atomic_write_replaces_a_file_and_removes_temporary_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    write_atomic(&path, "one").unwrap();
    write_atomic(&path, "two").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn guarded_atomic_write_keeps_original_and_removes_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "old").unwrap();

    let refused = write_atomic_guarded(&path, "new", &|| Err("changed".to_owned()));

    assert!(refused.unwrap_err().to_string().contains("changed"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn atomic_write_creates_the_directories_above_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("c.txt");

    write_atomic(&path, "deep").unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep");
}

#[test]
fn a_missing_file_reads_as_none_and_a_present_one_as_its_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    assert_eq!(read_optional(&path).unwrap(), None);

    std::fs::write(&path, "here").unwrap();

    assert_eq!(read_optional(&path).unwrap().as_deref(), Some("here"));
    assert!(read_optional(dir.path()).is_err(), "a directory is no text");
}

#[test]
fn slashed_spells_a_path_with_forward_slashes() {
    assert_eq!(slashed(std::path::Path::new("a/b.rs")), "a/b.rs");
    assert_eq!(slashed(std::path::Path::new("a\\b.rs")), "a/b.rs");
}

fn in_vars(base: Base, vars: &[(&str, &str)]) -> Option<PathBuf> {
    let vars: HashMap<&str, &str> = vars.iter().copied().collect();
    xdg_dir_in(base, |name| vars.get(name).map(OsString::from))
}

#[test]
fn each_base_defaults_below_home() {
    let home = [("HOME", "/home/ada")];
    assert_eq!(
        in_vars(Base::Config, &home),
        Some("/home/ada/.config/lighthouse".into())
    );
    assert_eq!(
        in_vars(Base::Data, &home),
        Some("/home/ada/.local/share/lighthouse".into())
    );
    assert_eq!(
        in_vars(Base::Cache, &home),
        Some("/home/ada/.cache/lighthouse".into())
    );
}

#[test]
fn an_absolute_variable_wins_over_home() {
    let vars = [("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "/etc/xdg")];
    assert_eq!(
        in_vars(Base::Config, &vars),
        Some("/etc/xdg/lighthouse".into())
    );
    assert_eq!(
        in_vars(Base::Data, &vars),
        Some("/home/ada/.local/share/lighthouse".into())
    );
}

#[test]
fn a_relative_variable_is_ignored() {
    let vars = [("HOME", "/home/ada"), ("XDG_CACHE_HOME", "relative/cache")];
    assert_eq!(
        in_vars(Base::Cache, &vars),
        Some("/home/ada/.cache/lighthouse".into())
    );
}

#[test]
fn nothing_usable_is_none() {
    assert_eq!(in_vars(Base::Config, &[]), None);
    assert_eq!(in_vars(Base::Config, &[("HOME", "home")]), None);
}

#[test]
fn dir() {
    let found = xdg_dir(Base::Data);
    assert!(found.is_none_or(|path| path.is_absolute() && path.ends_with("lighthouse")));
}

#[test]
fn plugins() {
    let found = xdg_plugins();
    assert!(found.is_none_or(|path| path.is_absolute() && path.ends_with("lighthouse/plugins")));
}
