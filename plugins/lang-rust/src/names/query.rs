//! Questions the body walker asks of the index: return types, methods,
//! associated constants and fields.

use super::*;

impl Index<'_> {
    /// The project type an `Result<T, _>` or `Option<T>` return type wraps, or
    /// the plain return type when `unwrap` is false.
    pub fn return_sym(&self, fn_id: &str, unwrap: bool) -> Option<Sym> {
        let sig = self.sigs.get(fn_id)?;
        let ret = sig.ret.as_ref()?;
        if !unwrap {
            return self.type_sym(ret, &sig.cx);
        }
        let syn::Type::Path(p) = ret else {
            return None;
        };
        let last = p.path.segments.last()?;
        if !matches!(last.ident.to_string().as_str(), "Result" | "Option") {
            return None;
        }
        self.type_sym(first_type_argument(last)?, &sig.cx)
    }

    /// The method `name` a value of type `recv` has: its own impls, then the
    /// default methods of the project traits it implements; a trait has its
    /// own methods.
    pub fn method(&self, recv: &Sym, name: &str) -> Option<String> {
        if let Some(entries) = self.assoc.get(&recv.id).and_then(|a| a.get(name)) {
            let methods: Vec<&Assoc> = entries
                .iter()
                .filter(|a| a.kind == SymbolKind::Method)
                .collect();
            if let Some(own) = methods.iter().find(|a| a.via_trait.is_none()) {
                return Some(own.id.clone());
            }
            let ids: BTreeSet<&str> = methods.iter().map(|a| a.id.as_str()).collect();
            if ids.len() == 1 {
                return ids.into_iter().next().map(str::to_owned);
            }
            if !ids.is_empty() {
                return None;
            }
        }
        if recv.kind == SymbolKind::Interface {
            return self
                .trait_methods
                .get(&recv.id)?
                .get(name)
                .map(|a| a.id.clone());
        }
        self.trait_impls
            .get(&recv.id)?
            .iter()
            .find_map(|t| self.trait_methods.get(&t.id)?.get(name))
            .map(|a| a.id.clone())
    }

    /// The associated constant `name` of a type.
    pub fn assoc_const(&self, recv: &Sym, name: &str) -> Option<String> {
        let entries = self.assoc.get(&recv.id)?.get(name)?;
        entries
            .iter()
            .find(|a| a.kind == SymbolKind::Const)
            .map(|a| a.id.clone())
    }

    pub fn field(&self, owner: &Sym, name: &str) -> Option<&Field> {
        self.fields.get(&owner.id)?.get(name)
    }
}
