//! The fragment of one source file: its modules, symbols, edges, function
//! summaries and test cases.

use std::collections::HashSet;

use lighthouse_protocol::{
    Edge, EdgeKind, FileInfo, Fragment, FunctionSummary, Module, Node, Resolution, Span, Symbol,
    SymbolKind, TestCase, Visibility,
};
use quote::ToTokens;
use syn::{Attribute, Block, Item};

use crate::{
    body::{self, Home},
    comments,
    names::{
        Generics, Index, ModId, Ns, Res, Sym, TyCx, ViaTrait, generics_of, is_test_fn, symbol_id,
    },
    testcase,
    tree::{SourceFile, Vis, vis},
    util::{count_tokens, doc_of, signature_counts, span_of},
};

/// What macros kept from the analysis in one file.
#[derive(Default)]
pub struct MacroStats {
    /// The file defines `macro_rules!` macros.
    pub defines: bool,
    /// Invocations that were not read: item position, or arguments that are
    /// not expressions.
    pub unread: u32,
}

pub fn fragment(idx: &Index, file: usize) -> (Fragment, MacroStats) {
    let src = &idx.tree.files[file];
    let mut out = Extractor {
        idx,
        file,
        src,
        frag: Fragment {
            file: FileInfo {
                path: src.rel.clone(),
                generated: src.generated,
            },
            ..Fragment::default()
        },
        seen: HashSet::new(),
        stats: MacroStats::default(),
    };
    for m in 0..idx.tree.mods.len() {
        if idx.tree.mods[m].file == file {
            out.module(m);
        }
    }
    out.frag.comments = comments::scan(&src.text, &out.frag.symbols);
    (out.frag, out.stats)
}

struct Extractor<'a> {
    idx: &'a Index<'a>,
    file: usize,
    src: &'a SourceFile,
    frag: Fragment,
    seen: HashSet<(EdgeKind, String, String, Resolution)>,
    stats: MacroStats,
}

/// A symbol to report.
struct Decl {
    id: String,
    kind: SymbolKind,
    visibility: Visibility,
    owner: Option<String>,
    span: Span,
    doc: Option<String>,
    name: String,
}

fn rank(v: Visibility) -> u8 {
    match v {
        Visibility::Public => 0,
        Visibility::Internal => 1,
        Visibility::Private => 2,
    }
}

/// The more restricted of two visibilities.
fn cap(a: Visibility, b: Visibility) -> Visibility {
    if rank(a) >= rank(b) { a } else { b }
}

fn declared(v: Vis) -> Visibility {
    match v {
        Vis::Pub => Visibility::Public,
        Vis::Crate => Visibility::Internal,
        Vis::Private => Visibility::Private,
    }
}

fn node_key(node: &Node) -> String {
    match node {
        Node::Module(path) => format!("module:{path}"),
        Node::Symbol(id) => format!("symbol:{id}"),
    }
}

impl Extractor<'_> {
    fn edge(&mut self, kind: EdgeKind, from: Node, to: String) {
        self.edge_as(kind, from, to, Resolution::Syntactic);
    }

    fn edge_as(&mut self, kind: EdgeKind, from: Node, to: String, resolution: Resolution) {
        if self
            .seen
            .insert((kind, node_key(&from), to.clone(), resolution))
        {
            self.frag.edges.push(Edge {
                kind,
                from,
                to,
                resolution,
            });
        }
    }

    fn declare(&mut self, d: Decl, container: Node) {
        self.edge(EdgeKind::Contains, container, d.id.clone());
        self.frag.symbols.push(Symbol {
            id: d.id,
            kind: d.kind,
            visibility: d.visibility,
            owner: d.owner,
            file: self.src.rel.clone(),
            span: d.span,
            doc: d.doc,
            name: d.name,
            role: None,
        });
    }

    fn module(&mut self, m: ModId) {
        let idx = self.idx;
        let module = &idx.tree.mods[m];
        let krate = &idx.tree.crates[module.krate];
        // An inline `#[cfg(test)]` module tests the module around it.
        let enclosing = module.parent.filter(|_| module.cfg_test);
        let test_of = match enclosing {
            Some(parent) => Some(idx.module_path(parent).to_owned()),
            None => krate.test_of.clone(),
        };
        self.frag.modules.push(Module {
            path: module.path.clone(),
            name: Some(module.name.clone()),
            test_of,
        });
        self.imports(m);
        for item in idx.tree.items(m) {
            self.item(m, item);
        }
    }

    fn imports(&mut self, m: ModId) {
        let idx = self.idx;
        let from = idx.module_path(m).to_owned();
        for entry in &idx.names[m].uses {
            if let Some(to) = self.import_target(m, &entry.path)
                && to != from
            {
                self.edge(EdgeKind::Imports, Node::Module(from.clone()), to);
            }
        }
    }

    /// The module an import depends on, or the external crate it names.
    fn import_target(&self, m: ModId, path: &[String]) -> Option<String> {
        let first = path.first()?;
        let found = self
            .idx
            .resolve_path(m, path, Ns::Type, &[])
            .or_else(|| self.idx.resolve_path(m, path, Ns::Value, &[]));
        match found {
            Some(Res::Module(target)) => Some(self.idx.module_path(target).to_owned()),
            Some(Res::Item(sym) | Res::Assoc(sym, _)) => {
                Some(self.idx.module_path(sym.module).to_owned())
            }
            Some(Res::External) => Some(first.clone()),
            None if !matches!(first.as_str(), "crate" | "self" | "super") => Some(first.clone()),
            None => None,
        }
    }

    fn item(&mut self, m: ModId, item: &Item) {
        match item {
            Item::Fn(f) => self.free_fn(m, f),
            Item::Struct(s) => {
                self.type_item(m, &s.ident, &s.vis, &s.attrs, s, Shape::Struct(&s.fields))
            }
            Item::Union(u) => {
                let fields = syn::Fields::Named(u.fields.clone());
                self.type_item(m, &u.ident, &u.vis, &u.attrs, u, Shape::Struct(&fields));
            }
            Item::Enum(e) => self.type_item(m, &e.ident, &e.vis, &e.attrs, e, Shape::Enum(e)),
            Item::Type(t) => self.type_item(m, &t.ident, &t.vis, &t.attrs, t, Shape::Plain),
            Item::Const(c) => self.value_item(m, &c.ident, &c.vis, &c.attrs, c, SymbolKind::Const),
            Item::Static(s) => self.value_item(m, &s.ident, &s.vis, &s.attrs, s, SymbolKind::Var),
            Item::Trait(t) => self.trait_item(m, t),
            Item::Impl(i) => self.impl_item(m, i),
            Item::Macro(mac) => self.item_macro(mac),
            _ => {}
        }
    }

    fn item_macro(&mut self, mac: &syn::ItemMacro) {
        if mac.mac.path.is_ident("macro_rules") {
            self.stats.defines = true;
        } else if !mac.mac.path.is_ident("include") {
            self.stats.unread += 1;
        }
    }

    fn free_fn(&mut self, m: ModId, f: &syn::ItemFn) {
        let name = f.sig.ident.to_string();
        let test = is_test_fn(&f.attrs);
        let kind = if test {
            SymbolKind::Test
        } else {
            SymbolKind::Function
        };
        let module = self.idx.module_path(m).to_owned();
        let id = symbol_id(&module, &[&name], kind);
        let visibility = self.idx.visibility(vis(&f.vis), m, &id);
        self.declare(
            Decl {
                id: id.clone(),
                kind,
                visibility,
                owner: None,
                span: span_of(self.src, f),
                doc: doc_of(&f.attrs),
                name: name.clone(),
            },
            Node::Module(module.clone()),
        );
        let cx = TyCx {
            module: m,
            self_ty: None,
            generics: generics_of(&f.sig.generics, None),
        };
        let home = Home {
            kind,
            module_path: module,
            parts: vec![name],
            cx,
        };
        self.summarize(
            &id,
            &home,
            &f.sig,
            &f.block,
            test.then_some(f.attrs.as_slice()),
        );
    }

    fn value_item(
        &mut self,
        m: ModId,
        ident: &syn::Ident,
        v: &syn::Visibility,
        attrs: &[Attribute],
        item: &impl ToTokens,
        kind: SymbolKind,
    ) {
        let name = ident.to_string();
        let module = self.idx.module_path(m).to_owned();
        let id = symbol_id(&module, &[&name], kind);
        let visibility = self.idx.visibility(vis(v), m, &id);
        self.declare(
            Decl {
                id,
                kind,
                visibility,
                owner: None,
                span: span_of(self.src, item),
                doc: doc_of(attrs),
                name,
            },
            Node::Module(module),
        );
    }

    fn type_item(
        &mut self,
        m: ModId,
        ident: &syn::Ident,
        v: &syn::Visibility,
        attrs: &[Attribute],
        item: &impl ToTokens,
        shape: Shape,
    ) {
        let name = ident.to_string();
        let module = self.idx.module_path(m).to_owned();
        let id = symbol_id(&module, &[&name], SymbolKind::Type);
        let visibility = self.idx.visibility(vis(v), m, &id);
        self.declare(
            Decl {
                id: id.clone(),
                kind: SymbolKind::Type,
                visibility,
                owner: None,
                span: span_of(self.src, item),
                doc: doc_of(attrs),
                name: name.clone(),
            },
            Node::Module(module.clone()),
        );
        let owner = Owner {
            id,
            name,
            module,
            visibility,
        };
        match shape {
            Shape::Struct(fields) => self.fields(&owner, fields),
            Shape::Enum(e) => self.variants(&owner, e),
            Shape::Plain => {}
        }
    }

    fn fields(&mut self, owner: &Owner, fields: &syn::Fields) {
        let syn::Fields::Named(named) = fields else {
            return;
        };
        for field in &named.named {
            let Some(ident) = &field.ident else {
                continue;
            };
            let name = ident.to_string();
            let id = symbol_id(&owner.module, &[&owner.name, &name], SymbolKind::Field);
            let visibility = cap(declared(vis(&field.vis)), owner.visibility);
            self.declare(
                Decl {
                    id,
                    kind: SymbolKind::Field,
                    visibility,
                    owner: Some(owner.id.clone()),
                    span: span_of(self.src, field),
                    doc: doc_of(&field.attrs),
                    name,
                },
                Node::Symbol(owner.id.clone()),
            );
        }
    }

    fn variants(&mut self, owner: &Owner, e: &syn::ItemEnum) {
        for variant in &e.variants {
            let name = variant.ident.to_string();
            let id = symbol_id(&owner.module, &[&owner.name, &name], SymbolKind::Field);
            self.declare(
                Decl {
                    id,
                    kind: SymbolKind::Field,
                    visibility: owner.visibility,
                    owner: Some(owner.id.clone()),
                    span: span_of(self.src, variant),
                    doc: doc_of(&variant.attrs),
                    name,
                },
                Node::Symbol(owner.id.clone()),
            );
        }
    }

    fn trait_item(&mut self, m: ModId, t: &syn::ItemTrait) {
        let name = t.ident.to_string();
        let module = self.idx.module_path(m).to_owned();
        let id = symbol_id(&module, &[&name], SymbolKind::Interface);
        let visibility = self.idx.visibility(vis(&t.vis), m, &id);
        self.declare(
            Decl {
                id: id.clone(),
                kind: SymbolKind::Interface,
                visibility,
                owner: None,
                span: span_of(self.src, t),
                doc: doc_of(&t.attrs),
                name: name.clone(),
            },
            Node::Module(module.clone()),
        );
        let self_ty = self.idx.names[m].types.get(&name).and_then(|d| match d {
            crate::names::Def::Sym(s) => Some(s.clone()),
            crate::names::Def::Module(..) => None,
        });
        let outer = generics_of(&t.generics, None);
        for item in &t.items {
            let syn::TraitItem::Fn(f) = item else {
                continue;
            };
            let method = f.sig.ident.to_string();
            let method_id = symbol_id(&module, &[&name, &method], SymbolKind::Method);
            self.declare(
                Decl {
                    id: method_id.clone(),
                    kind: SymbolKind::Method,
                    visibility,
                    owner: Some(id.clone()),
                    span: span_of(self.src, f),
                    doc: doc_of(&f.attrs),
                    name: method.clone(),
                },
                Node::Symbol(id.clone()),
            );
            let Some(block) = &f.default else {
                continue;
            };
            let home = Home {
                kind: SymbolKind::Method,
                module_path: module.clone(),
                parts: vec![name.clone(), method],
                cx: TyCx {
                    module: m,
                    self_ty: self_ty.clone(),
                    generics: generics_of(&f.sig.generics, Some(&outer)),
                },
            };
            self.summarize(&method_id, &home, &f.sig, block, None);
        }
    }

    fn impl_item(&mut self, m: ModId, i: &syn::ItemImpl) {
        let idx = self.idx;
        let generics = generics_of(&i.generics, None);
        let base_cx = TyCx {
            module: m,
            self_ty: None,
            generics: generics.clone(),
        };
        let owner = idx.impl_owner(&i.self_ty, &base_cx);
        let via = self.trait_of(m, i);
        let module = owner
            .as_ref()
            .map_or_else(|| idx.module_path(m), |o| idx.module_path(o.module))
            .to_owned();
        let parts = idx.impl_parts(i, owner.as_ref());
        let container = match &owner {
            Some(o) => Node::Symbol(o.id.clone()),
            None => Node::Module(module.clone()),
        };
        let ctx = ImplCtx {
            m,
            module,
            parts,
            owner: owner.clone(),
            via: via.clone(),
            generics,
            container,
            owner_vis: owner
                .as_ref()
                .map(|o| idx.visibility(o.vis, o.module, &o.id)),
        };
        let mut declared_any = false;
        for item in &i.items {
            declared_any |= self.impl_member(&ctx, i, item);
        }
        if let (Some(o), Some(ViaTrait::Project(t))) = (&owner, &via) {
            let anchored = declared_any || idx.tree.mods[o.module].file == self.file;
            if anchored {
                self.edge(
                    EdgeKind::Implements,
                    Node::Symbol(o.id.clone()),
                    t.id.clone(),
                );
            }
        }
    }

    fn trait_of(&self, m: ModId, i: &syn::ItemImpl) -> Option<ViaTrait> {
        let (_, path, _) = i.trait_.as_ref()?;
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        Some(match self.idx.resolve_path(m, &segments, Ns::Type, &[]) {
            Some(Res::Item(t)) if t.kind == SymbolKind::Interface => ViaTrait::Project(t),
            _ => ViaTrait::Foreign,
        })
    }

    /// Reports one impl item; true when a symbol was declared.
    fn impl_member(&mut self, ctx: &ImplCtx, i: &syn::ItemImpl, item: &syn::ImplItem) -> bool {
        match item {
            syn::ImplItem::Fn(f) => {
                self.impl_fn(ctx, f);
                true
            }
            syn::ImplItem::Const(c) if i.trait_.is_none() => {
                let name = c.ident.to_string();
                let mut ids: Vec<&str> = ctx.parts.iter().map(String::as_str).collect();
                ids.push(&name);
                let id = symbol_id(&ctx.module, &ids, SymbolKind::Const);
                let visibility = self.member_visibility(ctx, vis(&c.vis));
                self.declare(
                    Decl {
                        id,
                        kind: SymbolKind::Const,
                        visibility,
                        owner: ctx.owner.as_ref().map(|o| o.id.clone()),
                        span: span_of(self.src, c),
                        doc: doc_of(&c.attrs),
                        name,
                    },
                    ctx.container.clone(),
                );
                true
            }
            _ => false,
        }
    }

    /// Trait impl members are as visible as the trait and the type allow;
    /// inherent members as visible as written, capped by the type.
    fn member_visibility(&self, ctx: &ImplCtx, written: Vis) -> Visibility {
        if !self.idx.is_library(ctx.m) {
            return Visibility::Private;
        }
        let base = match &ctx.via {
            Some(ViaTrait::Project(t)) => self.idx.visibility(t.vis, t.module, &t.id),
            Some(ViaTrait::Foreign) => Visibility::Public,
            None => declared(written),
        };
        ctx.owner_vis.map_or(base, |o| cap(base, o))
    }

    fn impl_fn(&mut self, ctx: &ImplCtx, f: &syn::ImplItemFn) {
        let name = f.sig.ident.to_string();
        let mut parts = ctx.parts.clone();
        parts.push(name.clone());
        let ids: Vec<&str> = parts.iter().map(String::as_str).collect();
        let id = symbol_id(&ctx.module, &ids, SymbolKind::Method);
        let doc = doc_of(&f.attrs).or_else(|| self.inherited_doc(ctx, &name));
        let visibility = self.member_visibility(ctx, vis(&f.vis));
        self.declare(
            Decl {
                id: id.clone(),
                kind: SymbolKind::Method,
                visibility,
                owner: ctx.owner.as_ref().map(|o| o.id.clone()),
                span: span_of(self.src, f),
                doc,
                name,
            },
            ctx.container.clone(),
        );
        let home = Home {
            kind: SymbolKind::Method,
            module_path: ctx.module.clone(),
            parts,
            cx: TyCx {
                module: ctx.m,
                self_ty: ctx.owner.clone(),
                generics: generics_of(&f.sig.generics, Some(&ctx.generics)),
            },
        };
        self.summarize(&id, &home, &f.sig, &f.block, None);
    }

    /// A trait impl method without docs of its own shows the documentation of
    /// the trait's method, as rustdoc does.
    fn inherited_doc(&self, ctx: &ImplCtx, name: &str) -> Option<String> {
        let Some(ViaTrait::Project(t)) = &ctx.via else {
            return None;
        };
        let method = self.idx.trait_methods.get(&t.id)?.get(name)?;
        self.idx.trait_docs.get(&method.id).cloned()
    }

    /// Records the summary, the edges and, for tests, the test case of one
    /// function; functions declared in its body follow as symbols of their own.
    fn summarize(
        &mut self,
        id: &str,
        home: &Home,
        sig: &syn::Signature,
        block: &Block,
        test: Option<&[Attribute]>,
    ) {
        let facts = body::analyze(self.idx, home, sig, block);
        self.stats.unread += facts.unread_macros;
        for u in &facts.uses {
            self.edge_as(
                u.kind,
                Node::Symbol(id.to_owned()),
                u.to.clone(),
                u.resolution,
            );
        }
        let top_level = block
            .stmts
            .iter()
            .filter(|s| !matches!(s, syn::Stmt::Item(_)))
            .count();
        let (params, returns) = signature_counts(sig);
        self.frag.functions.push(FunctionSummary {
            symbol: id.to_owned(),
            max_nesting: facts.max_nesting,
            statements: facts.statements,
            top_level: u32::try_from(top_level).unwrap_or(u32::MAX),
            params,
            returns,
            tokens: count_tokens(block.to_token_stream()),
            flow: facts.flow,
            clone_fingerprint: None,
            forwards_to: facts.forwards_to,
            manual_assertions: 0,
        });
        if let Some(attrs) = test {
            self.frag.tests.push(TestCase {
                symbol: id.to_owned(),
                nesting: 0,
                style: testcase::style(attrs, block),
                targets: facts
                    .uses
                    .iter()
                    .filter(|u| u.resolution != Resolution::Heuristic)
                    .map(|u| u.to.clone())
                    .collect(),
            });
        }
        for nested in &facts.nested {
            self.nested_fn(id, home, nested);
        }
    }

    fn nested_fn(&mut self, outer: &str, home: &Home, f: &syn::ItemFn) {
        let name = f.sig.ident.to_string();
        let mut parts = home.parts.clone();
        parts.push(name.clone());
        let ids: Vec<&str> = parts.iter().map(String::as_str).collect();
        let id = symbol_id(&home.module_path, &ids, SymbolKind::Function);
        self.declare(
            Decl {
                id: id.clone(),
                kind: SymbolKind::Function,
                visibility: Visibility::Private,
                owner: Some(outer.to_owned()),
                span: span_of(self.src, f),
                doc: doc_of(&f.attrs),
                name,
            },
            Node::Symbol(outer.to_owned()),
        );
        let inner = Home {
            kind: SymbolKind::Function,
            module_path: home.module_path.clone(),
            parts,
            cx: TyCx {
                module: home.cx.module,
                self_ty: None,
                generics: generics_of(&f.sig.generics, None),
            },
        };
        self.summarize(&id, &inner, &f.sig, &f.block, None);
    }
}

enum Shape<'a> {
    Struct(&'a syn::Fields),
    Enum(&'a syn::ItemEnum),
    Plain,
}

struct Owner {
    id: String,
    name: String,
    module: String,
    visibility: Visibility,
}

struct ImplCtx {
    m: ModId,
    module: String,
    parts: Vec<String>,
    owner: Option<Sym>,
    via: Option<ViaTrait>,
    generics: Generics,
    container: Node,
    owner_vis: Option<Visibility>,
}
