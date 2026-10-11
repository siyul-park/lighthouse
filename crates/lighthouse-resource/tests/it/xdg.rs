use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use lighthouse_resource::{Base, xdg_dir, xdg_dir_in, xdg_plugins};

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
