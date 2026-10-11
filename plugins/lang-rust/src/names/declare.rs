//! Declaring: every module's items, fields, variants, traits and impl blocks.

use super::*;

impl Index<'_> {
    pub(super) fn declare(&mut self, m: ModId) {
        let tree = self.tree;
        for item in tree.items(m) {
            match item {
                Item::Fn(f) => self.declare_fn(m, f),
                Item::Struct(s) => self.declare_struct(m, s),
                Item::Enum(e) => self.declare_enum(m, e),
                Item::Union(u) => self.declare_union(m, u),
                Item::Trait(t) => self.declare_trait(m, t),
                Item::Type(t) => self.declare_alias(m, t),
                Item::Const(c) => {
                    let sym = self.sym(m, &[&c.ident.to_string()], SymbolKind::Const, vis(&c.vis));
                    self.names[m]
                        .values
                        .entry(sym.name.clone())
                        .or_insert(Def::Sym(sym));
                }
                Item::Static(s) => {
                    let sym = self.sym(m, &[&s.ident.to_string()], SymbolKind::Var, vis(&s.vis));
                    self.names[m]
                        .values
                        .entry(sym.name.clone())
                        .or_insert(Def::Sym(sym));
                }
                Item::Mod(d) => {
                    let name = d.ident.to_string();
                    if let Some(&child) = self.children.get(&(m, name.clone())) {
                        self.names[m]
                            .types
                            .entry(name)
                            .or_insert(Def::Module(child, vis(&d.vis)));
                    }
                }
                Item::Use(u) => {
                    let mut entries = Vec::new();
                    flatten(&u.tree, &mut Vec::new(), &mut entries);
                    for (path, name) in entries {
                        self.names[m].uses.push(UseEntry {
                            vis: vis(&u.vis),
                            path,
                            name,
                            module: m,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn declare_fn(&mut self, m: ModId, f: &syn::ItemFn) {
        let name = f.sig.ident.to_string();
        let kind = if is_test_fn(&f.attrs) {
            SymbolKind::Test
        } else {
            SymbolKind::Function
        };
        let sym = self.sym(m, &[&name], kind, vis(&f.vis));
        let cx = self.cx(m, None, generics_of(&f.sig.generics, None));
        self.sigs.insert(sym.id.clone(), signature(&f.sig, cx));
        self.names[m].values.entry(name).or_insert(Def::Sym(sym));
    }

    fn sym(&self, m: ModId, parts: &[&str], kind: SymbolKind, v: Vis) -> Sym {
        let name = parts.last().copied().unwrap_or("").to_owned();
        Sym {
            id: symbol_id(self.module_path(m), parts, kind),
            name,
            kind,
            module: m,
            vis: v,
        }
    }

    pub fn module_path(&self, m: ModId) -> &str {
        &self.tree.mods[m].path
    }

    fn cx(&self, m: ModId, self_ty: Option<Sym>, generics: Generics) -> TyCx {
        TyCx {
            module: m,
            self_ty,
            generics,
        }
    }

    fn declare_struct(&mut self, m: ModId, s: &syn::ItemStruct) {
        let name = s.ident.to_string();
        let sym = self.sym(m, &[&name], SymbolKind::Type, vis(&s.vis));
        let cx = self.cx(m, None, generics_of(&s.generics, None));
        self.declare_fields(&sym, &s.fields, &cx);
        if !matches!(s.fields, syn::Fields::Named(_)) {
            self.names[m]
                .values
                .entry(name.clone())
                .or_insert(Def::Sym(sym.clone()));
        }
        self.names[m].types.entry(name).or_insert(Def::Sym(sym));
    }

    fn declare_fields(&mut self, owner: &Sym, fields: &syn::Fields, cx: &TyCx) {
        let syn::Fields::Named(named) = fields else {
            return;
        };
        let table = self.fields.entry(owner.id.clone()).or_default();
        for field in &named.named {
            let Some(ident) = &field.ident else {
                continue;
            };
            let name = ident.to_string();
            let id = symbol_id(
                &self.tree.mods[owner.module].path,
                &[&owner.name, &name],
                SymbolKind::Field,
            );
            table.entry(name).or_insert(Field {
                ty: field.ty.clone(),
                cx: cx.clone(),
                id,
            });
        }
    }

    fn declare_enum(&mut self, m: ModId, e: &syn::ItemEnum) {
        let name = e.ident.to_string();
        let sym = self.sym(m, &[&name], SymbolKind::Type, vis(&e.vis));
        let table = self.variants.entry(sym.id.clone()).or_default();
        for variant in &e.variants {
            let v = variant.ident.to_string();
            let item = Sym {
                id: symbol_id(&self.tree.mods[m].path, &[&name, &v], SymbolKind::Variant),
                name: v.clone(),
                kind: SymbolKind::Variant,
                module: m,
                vis: sym.vis,
            };
            self.variant_owner.insert(item.id.clone(), sym.clone());
            table.entry(v).or_insert(item);
        }
        self.names[m].types.entry(name).or_insert(Def::Sym(sym));
    }

    fn declare_union(&mut self, m: ModId, u: &syn::ItemUnion) {
        let name = u.ident.to_string();
        let sym = self.sym(m, &[&name], SymbolKind::Type, vis(&u.vis));
        let cx = self.cx(m, None, generics_of(&u.generics, None));
        self.declare_fields(&sym, &syn::Fields::Named(u.fields.clone()), &cx);
        self.names[m].types.entry(name).or_insert(Def::Sym(sym));
    }

    fn declare_trait(&mut self, m: ModId, t: &syn::ItemTrait) {
        let name = t.ident.to_string();
        let sym = self.sym(m, &[&name], SymbolKind::Interface, vis(&t.vis));
        let generics = generics_of(&t.generics, None);
        let mut methods = BTreeMap::new();
        for item in &t.items {
            let syn::TraitItem::Fn(f) = item else {
                continue;
            };
            let method = f.sig.ident.to_string();
            let id = symbol_id(self.module_path(m), &[&name, &method], SymbolKind::Method);
            let cx = self.cx(
                m,
                Some(sym.clone()),
                generics_of(&f.sig.generics, Some(&generics)),
            );
            self.sigs.insert(id.clone(), signature(&f.sig, cx));
            if let Some(doc) = doc_of(&f.attrs) {
                self.trait_docs.insert(id.clone(), doc);
            }
            methods.entry(method).or_insert(Assoc {
                id,
                kind: SymbolKind::Method,
                via_trait: None,
            });
        }
        self.trait_methods.insert(sym.id.clone(), methods);
        self.names[m].types.entry(name).or_insert(Def::Sym(sym));
    }

    fn declare_alias(&mut self, m: ModId, t: &syn::ItemType) {
        let name = t.ident.to_string();
        let sym = self.sym(m, &[&name], SymbolKind::Type, vis(&t.vis));
        let cx = self.cx(m, None, generics_of(&t.generics, None));
        self.aliases.insert(sym.id.clone(), ((*t.ty).clone(), cx));
        self.names[m].types.entry(name).or_insert(Def::Sym(sym));
    }

    pub(super) fn impls(&mut self, m: ModId) {
        let tree = self.tree;
        for item in tree.items(m) {
            if let Item::Impl(i) = item {
                self.impl_block(m, i);
            }
        }
    }

    fn impl_block(&mut self, m: ModId, i: &syn::ItemImpl) {
        let generics = generics_of(&i.generics, None);
        let cx = self.cx(m, None, generics.clone());
        let owner = self.impl_owner(&i.self_ty, &cx);
        let via = i.trait_.as_ref().map(|(_, path, _)| {
            let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
            match self.resolve_path(m, &segments, Ns::Type, &[]) {
                Some(Res::Item(t)) if t.kind == SymbolKind::Interface => ViaTrait::Project(t),
                _ => ViaTrait::Foreign,
            }
        });
        if let (Some(owner), Some(ViaTrait::Project(t))) = (&owner, &via) {
            self.trait_impls
                .entry(owner.id.clone())
                .or_default()
                .push(t.clone());
        }
        let module = owner.as_ref().map_or(m, |o| o.module);
        let parts = self.impl_parts(i, owner.as_ref());
        for item in &i.items {
            let (name, kind, sig, declared) = match item {
                syn::ImplItem::Fn(f) => (
                    f.sig.ident.to_string(),
                    SymbolKind::Method,
                    Some(&f.sig),
                    vis(&f.vis),
                ),
                syn::ImplItem::Const(c) if i.trait_.is_none() => {
                    (c.ident.to_string(), SymbolKind::Const, None, vis(&c.vis))
                }
                _ => continue,
            };
            let mut ids: Vec<&str> = parts.iter().map(String::as_str).collect();
            ids.push(&name);
            let id = symbol_id(self.module_path(module), &ids, kind);
            if let Some(sig) = sig {
                let cx = self.cx(
                    m,
                    owner.clone(),
                    generics_of(&sig.generics, Some(&generics)),
                );
                self.sigs.insert(id.clone(), signature(sig, cx));
                if i.trait_.is_none() {
                    let public = declared == Vis::Pub;
                    self.inherent_methods
                        .entry(name.clone())
                        .or_default()
                        .push((id.clone(), module, public));
                }
            }
            if let Some(o) = &owner {
                self.assoc
                    .entry(o.id.clone())
                    .or_default()
                    .entry(name)
                    .or_default()
                    .push(Assoc {
                        id,
                        kind,
                        via_trait: via.clone(),
                    });
            }
        }
    }
}
