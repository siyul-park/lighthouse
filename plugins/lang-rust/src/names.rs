//! Names: what every module declares and imports, and syntactic resolution of
//! paths through the `mod` tree, `use` declarations (globs, renames, `self`,
//! `super`, `crate`) and the extern prelude of workspace crates.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use lighthouse_protocol::{SymbolKind, Visibility};
use quote::ToTokens;
use syn::{Item, UseTree};

use crate::{
    tree::{Tree, Vis, vis},
    util::doc_of,
};

/// Attributes that make a function a test, matched on the whole path:
/// `test`, `tokio::test`, `rstest`, `test_case::test_case`, ...
pub const TEST_ATTRIBUTES: [&str; 12] = [
    "test",
    "tokio::test",
    "async_std::test",
    "actix_rt::test",
    "actix_web::test",
    "sqlx::test",
    "wasm_bindgen_test::wasm_bindgen_test",
    "wasm_bindgen_test",
    "rstest",
    "rstest::rstest",
    "test_case",
    "test_case::test_case",
];

pub type ModId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ns {
    Type,
    Value,
}

/// A declared, nameable item of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sym {
    pub id: String,
    pub name: String,
    pub kind: SymbolKind,
    pub module: ModId,
    pub vis: Vis,
}

#[derive(Debug, Clone)]
pub enum Def {
    Module(ModId, Vis),
    Sym(Sym),
}

/// What a path denotes.
#[derive(Debug, Clone)]
pub enum Res {
    Module(ModId),
    Item(Sym),
    /// `Type::name` where `name` is not a variant: an associated item.
    Assoc(Sym, String),
    /// A crate or item outside the project.
    External,
}

#[derive(Debug, Clone)]
pub struct UseEntry {
    pub vis: Vis,
    pub path: Vec<String>,
    /// The bound name, `*` for a glob and `_` for an anonymous import.
    pub name: String,
    pub module: ModId,
}

#[derive(Default)]
pub struct ModNames {
    pub types: BTreeMap<String, Def>,
    pub values: BTreeMap<String, Def>,
    pub uses: Vec<UseEntry>,
}

/// Generic parameters in scope with the first trait bound of each.
pub type Generics = Rc<BTreeMap<String, Option<syn::Path>>>;

/// Where a type written in source is read: its module, the type `Self` stands
/// for and the generic parameters in scope.
#[derive(Clone)]
pub struct TyCx {
    pub module: ModId,
    pub self_ty: Option<Sym>,
    pub generics: Generics,
}

pub struct Sig {
    pub ret: Option<syn::Type>,
    pub cx: TyCx,
}

pub struct Assoc {
    pub id: String,
    pub kind: SymbolKind,
    pub via_trait: Option<ViaTrait>,
}

#[derive(Clone)]
pub enum ViaTrait {
    Project(Sym),
    Foreign,
}

pub struct Field {
    pub ty: syn::Type,
    pub cx: TyCx,
    pub id: String,
}

pub struct Index<'t> {
    pub tree: &'t Tree,
    pub names: Vec<ModNames>,
    pub externs: Vec<BTreeMap<String, ModId>>,
    pub fields: HashMap<String, BTreeMap<String, Field>>,
    pub variants: HashMap<String, BTreeMap<String, Sym>>,
    pub aliases: HashMap<String, (syn::Type, TyCx)>,
    pub assoc: HashMap<String, BTreeMap<String, Vec<Assoc>>>,
    pub trait_impls: HashMap<String, Vec<Sym>>,
    pub trait_methods: HashMap<String, BTreeMap<String, Assoc>>,
    pub sigs: HashMap<String, Sig>,
    pub variant_owner: HashMap<String, Sym>,
    pub trait_docs: HashMap<String, String>,
    /// Inherent methods that are not `pub`, by name: the candidates of a call
    /// whose receiver type is unknown.
    pub private_methods: HashMap<String, Vec<(String, ModId)>>,
    pub exported: HashSet<String>,
    pub exported_mods: HashSet<ModId>,
    children: HashMap<(ModId, String), ModId>,
}

impl<'t> Index<'t> {
    /// Declares every module's items, then its impl blocks, then settles
    /// which items are exported.
    pub fn build(tree: &'t Tree, packages: &[crate::cargo::Package]) -> Self {
        let mut children: HashMap<(ModId, String), ModId> = tree.aliases.clone();
        for (at, m) in tree.mods.iter().enumerate() {
            if let Some(parent) = m.parent {
                children.entry((parent, m.name.clone())).or_insert(at);
            }
        }
        let mut index = Self {
            tree,
            names: (0..tree.mods.len()).map(|_| ModNames::default()).collect(),
            externs: externs(tree, packages),
            fields: HashMap::new(),
            variants: HashMap::new(),
            aliases: HashMap::new(),
            assoc: HashMap::new(),
            trait_impls: HashMap::new(),
            trait_methods: HashMap::new(),
            sigs: HashMap::new(),
            variant_owner: HashMap::new(),
            trait_docs: HashMap::new(),
            private_methods: HashMap::new(),
            exported: HashSet::new(),
            exported_mods: HashSet::new(),
            children,
        };
        for m in 0..tree.mods.len() {
            index.declare(m);
        }
        for m in 0..tree.mods.len() {
            index.impls(m);
        }
        index.settle_exports();
        index
    }

    fn declare(&mut self, m: ModId) {
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
                id: symbol_id(&self.tree.mods[m].path, &[&name, &v], SymbolKind::Field),
                name: v.clone(),
                kind: SymbolKind::Field,
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

    fn impls(&mut self, m: ModId) {
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
                if i.trait_.is_none() && declared != Vis::Pub {
                    self.private_methods
                        .entry(name.clone())
                        .or_default()
                        .push((id.clone(), module));
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

    /// Marks everything `pub use` makes reachable from outside a library.
    fn settle_exports(&mut self) {
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
        self.tree.crates[self.tree.mods[m].krate].kind == crate::cargo::TargetKind::Lib
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
            if module.vis != Vis::Pub {
                return false;
            }
            at = parent;
        }
    }

    fn export_uses(&mut self, m: ModId) {
        let entries: Vec<UseEntry> = self.names[m]
            .uses
            .iter()
            .filter(|u| u.vis == Vis::Pub)
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
                Def::Sym(s) if s.vis == Vis::Pub => found.push(s.id.clone()),
                Def::Module(child, Vis::Pub) => modules.push(*child),
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
    pub fn visibility(&self, declared: Vis, m: ModId, id: &str) -> Visibility {
        if !self.is_library(m) {
            return Visibility::Private;
        }
        match declared {
            Vis::Private => Visibility::Private,
            Vis::Crate => Visibility::Internal,
            Vis::Pub if !self.tree.crates[self.tree.mods[m].krate].exported => Visibility::Internal,
            Vis::Pub if self.exported.contains(id) || self.module_reachable(m) => {
                Visibility::Public
            }
            Vis::Pub => Visibility::Internal,
        }
    }

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

pub fn symbol_id(module: &str, parts: &[&str], kind: SymbolKind) -> String {
    let mut id = module.to_owned();
    for part in parts {
        id.push_str("::");
        id.push_str(part);
    }
    id.push('#');
    id.push_str(kind_name(kind));
    id
}

pub fn kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Type => "type",
        SymbolKind::Field => "field",
        SymbolKind::Const => "const",
        SymbolKind::Var => "var",
        SymbolKind::Interface => "interface",
        SymbolKind::Test => "test",
    }
}

pub fn is_test_fn(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| TEST_ATTRIBUTES.contains(&crate::util::attr_path(a).as_str()))
}

pub fn generics_of(generics: &syn::Generics, outer: Option<&Generics>) -> Generics {
    let mut found: BTreeMap<String, Option<syn::Path>> =
        outer.map(|o| (**o).clone()).unwrap_or_default();
    for param in &generics.params {
        if let syn::GenericParam::Type(t) = param {
            found.insert(t.ident.to_string(), first_trait(t.bounds.iter()));
        }
    }
    if let Some(clause) = &generics.where_clause {
        for predicate in &clause.predicates {
            let syn::WherePredicate::Type(p) = predicate else {
                continue;
            };
            let syn::Type::Path(path) = &p.bounded_ty else {
                continue;
            };
            let Some(ident) = path.path.get_ident() else {
                continue;
            };
            if let Some(bound) = first_trait(p.bounds.iter()) {
                found.insert(ident.to_string(), Some(bound));
            }
        }
    }
    Rc::new(found)
}

pub fn first_trait<'a>(
    mut bounds: impl Iterator<Item = &'a syn::TypeParamBound>,
) -> Option<syn::Path> {
    bounds.find_map(|b| match b {
        syn::TypeParamBound::Trait(t) => Some(t.path.clone()),
        _ => None,
    })
}

/// `use` trees as `(path, bound name)` pairs.
pub fn flatten(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, String)>) {
    match tree {
        UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten(&p.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(n) => leaf(&n.ident.to_string(), None, prefix, out),
        UseTree::Rename(r) => leaf(
            &r.ident.to_string(),
            Some(r.rename.to_string()),
            prefix,
            out,
        ),
        UseTree::Glob(_) => out.push((prefix.clone(), "*".to_owned())),
        UseTree::Group(g) => {
            for item in &g.items {
                flatten(item, prefix, out);
            }
        }
    }
}

/// A type as source text without whitespace, for ids.
pub fn type_text(ty: &syn::Type) -> String {
    squash(&ty.to_token_stream().to_string())
}

/// The last segment of a path with its generic arguments, without whitespace.
pub fn path_text(path: &syn::Path) -> String {
    path.segments
        .last()
        .map_or_else(String::new, |s| squash(&s.to_token_stream().to_string()))
}

fn first_type_argument(segment: &syn::PathSegment) -> Option<&syn::Type> {
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

fn signature(sig: &syn::Signature, cx: TyCx) -> Sig {
    let ret = match &sig.output {
        syn::ReturnType::Type(_, ty) => Some((**ty).clone()),
        syn::ReturnType::Default => None,
    };
    Sig { ret, cx }
}

fn externs(tree: &Tree, packages: &[crate::cargo::Package]) -> Vec<BTreeMap<String, ModId>> {
    let libs: BTreeMap<&str, ModId> = tree
        .crates
        .iter()
        .filter(|c| c.kind == crate::cargo::TargetKind::Lib)
        .map(|c| (packages[c.package].name.as_str(), c.root))
        .collect();
    tree.crates
        .iter()
        .map(|c| {
            let package = &packages[c.package];
            let mut found = BTreeMap::new();
            for (extern_name, real) in &package.deps {
                if let Some(&root) = libs.get(real.as_str()) {
                    found.insert(extern_name.clone(), root);
                }
            }
            if c.kind != crate::cargo::TargetKind::Lib
                && let Some(&root) = libs.get(package.name.as_str())
            {
                found.insert(package.lib_name.clone(), root);
            }
            found
        })
        .collect()
}

fn leaf(
    ident: &str,
    rename: Option<String>,
    prefix: &[String],
    out: &mut Vec<(Vec<String>, String)>,
) {
    let mut path = prefix.to_vec();
    if ident != "self" {
        path.push(ident.to_owned());
    }
    let name = rename.or_else(|| path.last().cloned());
    if let Some(name) = name {
        out.push((path, name));
    }
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
