//! What is reachable from outside a library, and so how visible an item is.

use lighthouse_protocol::Visibility;

use crate::names::{Def, Index, ModId, Ns, Res, UseEntry};

impl Index<'_> {
    /// Marks everything `pub use` makes reachable from outside a library.
    pub(crate) fn settle_exports(&mut self) {
        loop {
            let before = (self.exported.len(), self.exported_mods.len());
            for m in 0..self.tree.mods.len() {
                if self.is_library(m) && self.module_reachable(m) {
                    self.export_uses(m);
                }
            }
            if before == (self.exported.len(), self.exported_mods.len()) {
                break;
            }
        }
    }

    pub fn is_library(&self, m: ModId) -> bool {
        self.tree.crates[self.tree.mods[m].krate].kind == crate::project::cargo::TargetKind::Lib
    }

    /// Every module from the crate root down to `m` is `pub` or re-exported.
    pub fn module_reachable(&self, m: ModId) -> bool {
        let mut at = m;
        loop {
            let module = &self.tree.mods[at];
            let Some(parent) = module.parent else {
                return true;
            };
            // A re-exported module is reachable by its new name, whatever
            // the modules around its declaration are.
            if self.exported_mods.contains(&at) {
                return true;
            }
            if module.vis != Visibility::Public {
                return false;
            }
            at = parent;
        }
    }

    fn export_uses(&mut self, m: ModId) {
        let entries: Vec<UseEntry> = self.names[m]
            .uses
            .iter()
            .filter(|u| u.vis == Visibility::Public)
            .cloned()
            .collect();
        for entry in entries {
            if entry.name == "*" {
                if let Some(Res::Module(target)) = self.resolve_path(m, &entry.path, Ns::Type, &[])
                {
                    self.export_declared(target);
                }
                continue;
            }
            for ns in [Ns::Type, Ns::Value] {
                match self.resolve_path(m, &entry.path, ns, &[]) {
                    Some(Res::Item(sym)) => {
                        self.exported.insert(sym.id);
                    }
                    Some(Res::Module(target)) => {
                        self.exported_mods.insert(target);
                    }
                    _ => {}
                }
            }
        }
    }

    fn export_declared(&mut self, target: ModId) {
        let mut found: Vec<String> = Vec::new();
        let mut modules = Vec::new();
        let names = &self.names[target];
        for def in names.types.values().chain(names.values.values()) {
            match def {
                Def::Sym(s) if s.vis == Visibility::Public => found.push(s.id.clone()),
                Def::Module(child, Visibility::Public) => modules.push(*child),
                _ => {}
            }
        }
        self.exported.extend(found);
        self.exported_mods.extend(modules);
    }

    /// Effective visibility of a module-level item: what its declaration says,
    /// capped by whether anything outside the crate (or the project) can reach
    /// it. Binary, test and example crates export nothing; the public items of
    /// a library that cannot be published are importable only inside the
    /// project, like Go's `internal` packages.
    pub fn visibility(&self, declared: Visibility, m: ModId, id: &str) -> Visibility {
        if !self.is_library(m) {
            return Visibility::Private;
        }
        match declared {
            Visibility::Private => Visibility::Private,
            Visibility::Internal => Visibility::Internal,
            Visibility::Public if !self.tree.crates[self.tree.mods[m].krate].exported => {
                Visibility::Internal
            }
            Visibility::Public if self.exported.contains(id) || self.module_reachable(m) => {
                Visibility::Public
            }
            Visibility::Public => Visibility::Internal,
        }
    }
}
