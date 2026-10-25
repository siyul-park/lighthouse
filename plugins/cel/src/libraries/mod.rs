//! CEL functions contributed by the bundled plugin, one library per module.

pub mod glob;

mod bind;
mod lists;
mod order;
mod regex;

use lighthouse_plugin::CelLibrary;

pub(crate) fn all() -> Vec<Box<dyn CelLibrary>> {
    [
        regex::library(),
        lists::library(),
        glob::library(),
        bind::library(),
        order::library(),
    ]
    .into_iter()
    .map(|library| Box::new(library) as Box<dyn CelLibrary>)
    .collect()
}
