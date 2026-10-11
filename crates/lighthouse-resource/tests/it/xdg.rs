use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use lighthouse_resource::{Base, xdg_dir_in};

fn dir(base: Base, vars: &[(&str, &str)]) -> Option<PathBuf> {
    let vars: HashMap<&str, &str> = vars.iter().copied().collect();
    xdg_dir_in(base, |name| vars.get(name).map(OsString::from))
}

#[test]
fn each_base_defaults_below_home() {
    let home = [("HOME", "/home/ada")];
    assert_eq!(
        dir(Base::Config, &home),
        Some("/home/ada/.config/lighthouse".into())
    );
    assert_eq!(
        dir(Base::Data, &home),
        Some("/home/ada/.local/share/lighthouse".into())
    );
    assert_eq!(
        dir(Base::Cache, &home),
        Some("/home/ada/.cache/lighthouse".into())
    );
}

#[test]
fn an_absolute_variable_wins_over_home() {
    let vars = [("HOME", "/home/ada"), ("XDG_CONFIG_HOME", "/etc/xdg")];
    assert_eq!(dir(Base::Config, &vars), Some("/etc/xdg/lighthouse".into()));
    assert_eq!(
        dir(Base::Data, &vars),
        Some("/home/ada/.local/share/lighthouse".into())
    );
}

#[test]
fn a_relative_variable_is_ignored() {
    let vars = [("HOME", "/home/ada"), ("XDG_CACHE_HOME", "relative/cache")];
    assert_eq!(
        dir(Base::Cache, &vars),
        Some("/home/ada/.cache/lighthouse".into())
    );
}

#[test]
fn nothing_usable_is_none() {
    assert_eq!(dir(Base::Config, &[]), None);
    assert_eq!(dir(Base::Config, &[("HOME", "home")]), None);
}
