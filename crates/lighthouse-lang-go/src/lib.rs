mod body;
mod declarations;
mod imports;
mod locals;
mod provider;
mod testcase;

use lighthouse_plugin::{LanguageProvider, Manifest, Plugin};

pub use provider::Go;

pub const ID: &str = "lang-go";

pub struct LangGo;

impl Plugin for LangGo {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: ID.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        vec![Box::new(Go::new())]
    }
}
