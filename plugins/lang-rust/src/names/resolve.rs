//! Resolving paths and looking members up through the declared names.

use super::*;

impl Index<'_> {
    /// The project type an impl block is for. A generic parameter (a blanket
    /// impl) or a type outside the project has no owner.
    pub fn impl_owner(&self, ty: &syn::Type, cx: &TyCx) -> Option<Sym> {
        if let syn::Type::Path(p) = ty
            && p.path
                .get_ident()
                .is_some_and(|i| cx.generics.contains_key(&i.to_string()))
        {
            return None;
        }
        self.type_sym(ty, cx)
    }

    /// The project type or trait a written type denotes, behind references,
    /// `Box`/`Arc`/`Rc`, `dyn Trait`, `impl Trait` and generic parameters.
    pub fn type_sym(&self, ty: &syn::Type, cx: &TyCx) -> Option<Sym> {
        match ty {
            syn::Type::Reference(r) => self.type_sym(&r.elem, cx),
            syn::Type::Paren(p) => self.type_sym(&p.elem, cx),
            syn::Type::Group(g) => self.type_sym(&g.elem, cx),
            syn::Type::TraitObject(t) => self.bound_sym(first_trait(t.bounds.iter())?, cx),
            syn::Type::ImplTrait(t) => self.bound_sym(first_trait(t.bounds.iter())?, cx),
            syn::Type::Path(p) if p.qself.is_none() => self.path_type_sym(&p.path, cx),
            _ => None,
        }
    }

    fn bound_sym(&self, path: syn::Path, cx: &TyCx) -> Option<Sym> {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match self.resolve_path(cx.module, &segments, Ns::Type, &[])? {
            Res::Item(sym) if sym.kind == SymbolKind::Interface => Some(sym),
            _ => None,
        }
    }

    pub fn resolve_path(
        &self,
        m: ModId,
        segs: &[String],
        ns: Ns,
        extra: &[UseEntry],
    ) -> Option<Res> {
        self.resolve_with(m, segs, ns, extra, &mut Vec::new())
    }

    fn resolve_with(
        &self,
        m: ModId,
        segs: &[String],
        ns: Ns,
        extra: &[UseEntry],
        seen: &mut Vec<(ModId, String, Ns)>,
    ) -> Option<Res> {
        let (first, rest) = segs.split_first()?;
        let lead_ns = if rest.is_empty() { ns } else { Ns::Type };
        let mut current = match first.as_str() {
            "crate" => Res::Module(self.crate_root(m)),
            "self" => Res::Module(m),
            "super" => Res::Module(self.tree.mods[m].parent?),
            "Self" => return None,
            name => self.first(m, name, lead_ns, extra, seen)?,
        };
        for (at, seg) in rest.iter().enumerate() {
            let step_ns = if at + 1 == rest.len() { ns } else { Ns::Type };
            current = self.step(current, seg, step_ns, seen)?;
        }
        Some(current)
    }

    pub fn crate_root(&self, m: ModId) -> ModId {
        self.tree.crates[self.tree.mods[m].krate].root
    }

    fn first(
        &self,
        m: ModId,
        name: &str,
        ns: Ns,
        extra: &[UseEntry],
        seen: &mut Vec<(ModId, String, Ns)>,
    ) -> Option<Res> {
        for entry in extra.iter().filter(|u| u.name == name) {
            if let Some(found) = self.resolve_with(entry.module, &entry.path, ns, &[], seen) {
                return Some(found);
            }
        }
        if let Some(found) = self.lookup_in_module(m, name, ns, seen) {
            return Some(found);
        }
        let krate = self.tree.mods[m].krate;
        if let Some(&root) = self.externs[krate].get(name) {
            return Some(Res::Module(root));
        }
        matches!(name, "std" | "core" | "alloc").then_some(Res::External)
    }

    pub fn lookup_in_module(
        &self,
        m: ModId,
        name: &str,
        ns: Ns,
        seen: &mut Vec<(ModId, String, Ns)>,
    ) -> Option<Res> {
        let key = (m, name.to_owned(), ns);
        if seen.contains(&key) {
            return None;
        }
        seen.push(key);
        let names = &self.names[m];
        let table = match ns {
            Ns::Type => &names.types,
            Ns::Value => &names.values,
        };
        if let Some(def) = table.get(name) {
            return Some(match def {
                Def::Module(child, _) => Res::Module(*child),
                Def::Sym(sym) => Res::Item(sym.clone()),
            });
        }
        for entry in names.uses.iter().filter(|u| u.name == name) {
            if let Some(found) = self.resolve_with(entry.module, &entry.path, ns, &[], seen) {
                return Some(found);
            }
        }
        for entry in names.uses.iter().filter(|u| u.name == "*") {
            let found = match self.resolve_with(entry.module, &entry.path, Ns::Type, &[], seen) {
                Some(Res::Module(target)) => self.lookup_in_module(target, name, ns, seen),
                Some(Res::Item(e)) => self
                    .variants
                    .get(&e.id)
                    .and_then(|v| v.get(name))
                    .cloned()
                    .map(Res::Item),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn step(
        &self,
        current: Res,
        seg: &str,
        ns: Ns,
        seen: &mut Vec<(ModId, String, Ns)>,
    ) -> Option<Res> {
        match current {
            Res::Module(at) if seg == "super" => Some(Res::Module(self.tree.mods[at].parent?)),
            Res::Module(at) => self.lookup_in_module(at, seg, ns, seen),
            Res::Item(sym) if matches!(sym.kind, SymbolKind::Type | SymbolKind::Interface) => {
                Some(self.member(sym, seg))
            }
            Res::External => Some(Res::External),
            _ => None,
        }
    }

    /// `sym::seg`: a variant of an enum, else an associated item.
    pub fn member(&self, sym: Sym, seg: &str) -> Res {
        let sym = self.canonical(sym);
        match self.variants.get(&sym.id).and_then(|v| v.get(seg)) {
            Some(variant) => Res::Item(variant.clone()),
            None => Res::Assoc(sym, seg.to_owned()),
        }
    }

    /// Follows type aliases to the type they name.
    pub fn canonical(&self, sym: Sym) -> Sym {
        let mut sym = sym;
        for _ in 0..8 {
            let Some((ty, cx)) = self.aliases.get(&sym.id) else {
                break;
            };
            match self.type_sym(ty, cx) {
                Some(next) => sym = next,
                None => break,
            }
        }
        sym
    }

    fn path_type_sym(&self, path: &syn::Path, cx: &TyCx) -> Option<Sym> {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        if let [only] = segments.as_slice() {
            if only == "Self" {
                return cx.self_ty.clone();
            }
            if let Some(bound) = cx.generics.get(only) {
                return self.bound_sym(bound.clone()?, cx);
            }
        }
        let last = path.segments.last()?;
        if matches!(last.ident.to_string().as_str(), "Box" | "Arc" | "Rc")
            && let Some(inner) = first_type_argument(last)
        {
            return self.type_sym(inner, cx);
        }
        match self.resolve_path(cx.module, &segments, Ns::Type, &[])? {
            Res::Item(sym) if matches!(sym.kind, SymbolKind::Type | SymbolKind::Interface) => {
                Some(self.canonical(sym))
            }
            _ => None,
        }
    }

    /// Owner path parts of an impl's items: the type, then for a trait impl
    /// the trait with its arguments, so two impls never share an id.
    pub fn impl_parts(&self, i: &syn::ItemImpl, owner: Option<&Sym>) -> Vec<String> {
        let mut parts = vec![owner.map_or_else(|| type_text(&i.self_ty), |o| o.name.clone())];
        if let Some((_, path, _)) = &i.trait_ {
            parts.push(path_text(path));
        }
        parts
    }
}
