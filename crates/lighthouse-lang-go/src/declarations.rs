use std::path::Path;

use lighthouse_model::{
    Edge, EdgeKind, Node, Resolution, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use lighthouse_syntax::{Match, Node as Ast, children_named, span, text};

/// Where a function body is analyzed from.
pub(crate) struct Function<'t> {
    pub id: SymbolId,
    pub node: Ast<'t>,
    pub receiver: Option<Receiver>,
    pub name: String,
    pub test: bool,
}

pub(crate) struct Receiver {
    pub name: Option<String>,
    pub type_name: String,
}

#[derive(Default)]
pub(crate) struct Declared<'t> {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
    pub functions: Vec<Function<'t>>,
}

pub(crate) struct Site<'a> {
    pub module: &'a str,
    pub file: &'a Path,
    pub source: &'a str,
    pub test_file: bool,
}

impl<'t> Declared<'t> {
    pub(crate) fn collect(site: &Site, matches: &[Match<'t>]) -> Self {
        let mut declared = Self::default();
        for m in matches {
            let group = m.get("group");
            if let Some(node) = m.get("function") {
                declared.function(site, node);
            } else if let Some(node) = m.get("method") {
                declared.method(site, node);
            } else if let Some(node) = m.get("type") {
                declared.type_spec(site, node, group);
            } else if let Some(node) = m.get("const") {
                declared.values(site, node, group, SymbolKind::Const);
            } else if let Some(node) = m.get("var") {
                declared.values(site, node, group, SymbolKind::Var);
            }
        }
        declared
    }

    fn function(&mut self, site: &Site, node: Ast<'t>) {
        let Some(name) = field_text(node, "name", site.source) else {
            return;
        };
        let is_test = site.test_file && is_test_name(name);
        let kind = if is_test {
            SymbolKind::Test
        } else {
            SymbolKind::Function
        };
        let id_name = if name == "init" {
            format!(
                "init:{}:{}",
                file_name(site.file),
                node.start_position().row + 1
            )
        } else {
            name.to_owned()
        };
        let id = SymbolId::new(site.module, &[], &id_name, kind);
        self.push(site, Entry::new(id.clone(), kind, node, name));
        self.functions.push(Function {
            id,
            node,
            receiver: None,
            name: name.to_owned(),
            test: is_test,
        });
    }

    fn method(&mut self, site: &Site, node: Ast<'t>) {
        let Some(name) = field_text(node, "name", site.source) else {
            return;
        };
        let Some(receiver) = receiver(node, site.source) else {
            return;
        };
        let owner = SymbolId::new(site.module, &[], &receiver.type_name, SymbolKind::Type);
        let id = SymbolId::new(
            site.module,
            &[&receiver.type_name],
            name,
            SymbolKind::Method,
        );
        let entry = Entry::new(id.clone(), SymbolKind::Method, node, name);
        self.push(
            site,
            Entry {
                owner: Some(owner),
                ..entry
            },
        );
        self.functions.push(Function {
            id,
            node,
            receiver: Some(receiver),
            name: name.to_owned(),
            test: false,
        });
    }

    fn type_spec(&mut self, site: &Site, node: Ast<'t>, group: Option<Ast<'t>>) {
        let Some(name) = field_text(node, "name", site.source) else {
            return;
        };
        let interface = node
            .child_by_field_name("type")
            .is_some_and(|t| t.kind() == "interface_type");
        let kind = if interface {
            SymbolKind::Interface
        } else {
            SymbolKind::Type
        };
        let id = SymbolId::new(site.module, &[], name, kind);
        let entry = Entry::new(id.clone(), kind, node, name);
        self.push(site, Entry { group, ..entry });
        let Some(body) = node.child_by_field_name("type") else {
            return;
        };
        if body.kind() == "struct_type" {
            for list in children_named(body, "field_declaration_list") {
                for field in children_named(list, "field_declaration") {
                    self.field(site, &id, name, field);
                }
            }
        }
    }

    fn field(&mut self, site: &Site, owner: &SymbolId, owner_name: &str, node: Ast<'t>) {
        let mut cursor = node.walk();
        let names: Vec<&str> = node
            .children_by_field_name("name", &mut cursor)
            .map(|n| text(n, site.source))
            .collect();
        let embedded;
        let names = if names.is_empty() {
            let Some(kind) = node.child_by_field_name("type") else {
                return;
            };
            embedded = embedded_name(kind, site.source);
            vec![embedded.as_str()]
        } else {
            names
        };
        for name in names.into_iter().filter(|n| *n != "_") {
            let id = SymbolId::new(site.module, &[owner_name], name, SymbolKind::Field);
            let entry = Entry::new(id, SymbolKind::Field, node, name);
            self.push(
                site,
                Entry {
                    owner: Some(owner.clone()),
                    ..entry
                },
            );
        }
    }

    fn values(&mut self, site: &Site, node: Ast<'t>, group: Option<Ast<'t>>, kind: SymbolKind) {
        let mut cursor = node.walk();
        let names: Vec<Ast> = node.children_by_field_name("name", &mut cursor).collect();
        for name in names.into_iter().filter(|n| n.kind() == "identifier") {
            let name = text(name, site.source);
            if name == "_" {
                continue;
            }
            let id = SymbolId::new(site.module, &[], name, kind);
            let entry = Entry::new(id, kind, node, name);
            self.push(site, Entry { group, ..entry });
        }
    }

    fn push(&mut self, site: &Site, entry: Entry<'_, 't>) {
        let from = entry
            .owner
            .clone()
            .map_or_else(|| Node::Module(site.module.to_owned()), Node::Symbol);
        self.edges.push(Edge {
            kind: EdgeKind::Contains,
            from,
            to: Target::Resolved(Node::Symbol(entry.id.clone())),
            resolution: Resolution::Syntactic,
        });
        let doc =
            doc(entry.node, site.source).or_else(|| entry.group.and_then(|g| doc(g, site.source)));
        self.symbols.push(Symbol {
            id: entry.id,
            kind: entry.kind,
            visibility: visibility(entry.name),
            owner: entry.owner,
            file: site.file.to_owned(),
            span: span(entry.node),
            doc,
            name: entry.name.to_owned(),
        });
    }
}

struct Entry<'a, 't> {
    id: SymbolId,
    kind: SymbolKind,
    owner: Option<SymbolId>,
    node: Ast<'t>,
    group: Option<Ast<'t>>,
    name: &'a str,
}

impl<'a, 't> Entry<'a, 't> {
    fn new(id: SymbolId, kind: SymbolKind, node: Ast<'t>, name: &'a str) -> Self {
        Self {
            id,
            kind,
            owner: None,
            node,
            group: None,
            name,
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn field_text<'s>(node: Ast, field: &str, source: &'s str) -> Option<&'s str> {
    node.child_by_field_name(field).map(|n| text(n, source))
}

pub(crate) fn visibility(name: &str) -> Visibility {
    if name.chars().next().is_some_and(char::is_uppercase) {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

/// `Test`, `Benchmark`, `Fuzz` and `Example` entry points of `go test`.
fn is_test_name(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.starts_with(char::is_lowercase))
        })
}

pub(crate) fn is_test_case_name(name: &str) -> bool {
    name.strip_prefix("Test")
        .is_some_and(|rest| !rest.starts_with(char::is_lowercase))
}

fn receiver(node: Ast, source: &str) -> Option<Receiver> {
    let list = node.child_by_field_name("receiver")?;
    let param = children_named(list, "parameter_declaration")
        .into_iter()
        .next()?;
    let name = field_text(param, "name", source)
        .filter(|n| *n != "_")
        .map(str::to_owned);
    let type_name = base_type_name(param.child_by_field_name("type")?, source)?;
    Some(Receiver { name, type_name })
}

/// Name of the declared type behind pointers and type arguments.
pub(crate) fn base_type_name(node: Ast, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, source).to_owned()),
        "pointer_type" | "parenthesized_type" => base_type_name(node.named_child(0)?, source),
        "generic_type" => base_type_name(node.child_by_field_name("type")?, source),
        _ => None,
    }
}

fn embedded_name(node: Ast, source: &str) -> String {
    match node.kind() {
        "qualified_type" => field_text(node, "name", source)
            .unwrap_or_default()
            .to_owned(),
        _ => base_type_name(node, source).unwrap_or_default(),
    }
}

fn doc(node: Ast, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut row = node.start_position().row;
    let mut prev = node.prev_sibling();
    while let Some(comment) = prev {
        if comment.kind() != "comment" || comment.end_position().row + 1 < row || trailing(comment)
        {
            break;
        }
        lines.push(strip(text(comment, source)));
        row = comment.start_position().row;
        prev = comment.prev_sibling();
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

fn trailing(comment: Ast) -> bool {
    comment.prev_sibling().is_some_and(|p| {
        p.kind() != "comment" && p.end_position().row == comment.start_position().row
    })
}

fn strip(comment: &str) -> String {
    let body = comment
        .strip_prefix("//")
        .or_else(|| {
            comment
                .strip_prefix("/*")
                .map(|c| c.strip_suffix("*/").unwrap_or(c))
        })
        .unwrap_or(comment);
    body.strip_prefix(' ').unwrap_or(body).trim_end().to_owned()
}
