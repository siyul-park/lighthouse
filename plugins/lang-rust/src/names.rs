//! Names: what every module declares and imports, and syntactic resolution of
//! paths through the `mod` tree, `use` declarations (globs, renames, `self`,
//! `super`, `crate`) and the extern prelude of workspace crates.

mod declare;
mod query;
mod resolve;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    rc::Rc,
};

use lighthouse_protocol::{Span, SymbolKind, Visibility};
use quote::ToTokens;
use syn::{Item, UseTree};

use crate::{
    extent::span_of,
    project::{Tree, vis},
    syntax::doc_of,
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
    pub vis: Visibility,
}

#[derive(Debug, Clone)]
pub enum Def {
    Module(ModId, Visibility),
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
    pub vis: Visibility,
    pub path: Vec<String>,
    /// The bound name, `*` for a glob and `_` for an anonymous import.
    pub name: String,
    pub module: ModId,
    /// The `use` item the entry comes from, as written.
    pub span: Span,
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
    /// Inherent methods by name, with whether each is `pub`: the candidates of
    /// a call whose receiver type is unknown.
    pub inherent_methods: HashMap<String, Vec<(String, ModId, bool)>>,
    pub exported: HashSet<String>,
    pub exported_mods: HashSet<ModId>,
    children: HashMap<(ModId, String), ModId>,
}

impl<'t> Index<'t> {
    /// Declares every module's items, then its impl blocks, then settles
    /// which items are exported.
    pub fn build(tree: &'t Tree, packages: &[crate::project::cargo::Package]) -> Self {
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
            inherent_methods: HashMap::new(),
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
        SymbolKind::Variant => "variant",
        SymbolKind::Const => "const",
        SymbolKind::Var => "var",
        SymbolKind::Interface => "interface",
        SymbolKind::Test => "test",
    }
}

pub fn is_test_fn(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| TEST_ATTRIBUTES.contains(&crate::syntax::attr_path(a).as_str()))
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

fn externs(
    tree: &Tree,
    packages: &[crate::project::cargo::Package],
) -> Vec<BTreeMap<String, ModId>> {
    let libs: BTreeMap<&str, ModId> = tree
        .crates
        .iter()
        .filter(|c| c.kind == crate::project::cargo::TargetKind::Lib)
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
            if c.kind != crate::project::cargo::TargetKind::Lib
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
