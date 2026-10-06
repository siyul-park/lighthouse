//! Source files and the module tree of every crate target: crate roots, `mod`
//! declarations with their file lookup rules (`name.rs`, `name/mod.rs`,
//! `#[path]`, inline modules). Nothing is evaluated: every `cfg` branch is
//! followed, and only `cfg(test)` is remembered.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use proc_macro2::{TokenStream, TokenTree};
use syn::{
    Attribute, Expr, ExprLit, Item, Lit, Meta, Token, Visibility, parse::Parser,
    punctuated::Punctuated,
};

use crate::cargo::TargetKind;

/// Declared visibility, before it is capped by the surrounding modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    Pub,
    Crate,
    Private,
}

pub fn vis(v: &Visibility) -> Vis {
    match v {
        Visibility::Public(_) => Vis::Pub,
        Visibility::Restricted(r) if r.path.is_ident("self") => Vis::Private,
        Visibility::Restricted(_) => Vis::Crate,
        Visibility::Inherited => Vis::Private,
    }
}

pub struct SourceFile {
    pub rel: String,
    pub text: String,
    pub ast: syn::File,
    pub generated: bool,
    line_starts: Vec<usize>,
}

impl SourceFile {
    /// 1-based line and byte column of a proc-macro2 position (0-based
    /// character column).
    pub fn position(&self, line: usize, col: usize) -> (u32, u32) {
        let Some(&start) = self.line_starts.get(line.wrapping_sub(1)) else {
            return (u32::try_from(line).unwrap_or(u32::MAX), 1);
        };
        let rest = &self.text[start..];
        let line_text = rest.split('\n').next().unwrap_or("");
        let bytes = line_text
            .char_indices()
            .nth(col)
            .map_or(line_text.len(), |(at, _)| at);
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(bytes + 1).unwrap_or(u32::MAX),
        )
    }
}

pub struct Crate {
    pub package: usize,
    pub kind: TargetKind,
    pub root: usize,
    /// Whether code outside the project can depend on the package.
    pub exported: bool,
    /// Label of the library crate an integration test crate exercises.
    pub test_of: Option<String>,
}

pub struct Mod {
    pub path: String,
    pub name: String,
    pub parent: Option<usize>,
    pub krate: usize,
    pub file: usize,
    /// Indexes of the nested inline `mod` items leading from the file's items
    /// to this module's items.
    pub inline: Vec<usize>,
    pub vis: Vis,
    pub cfg_test: bool,
    dir: PathBuf,
    nested: bool,
}

#[derive(Default)]
pub struct Tree {
    pub files: Vec<SourceFile>,
    pub crates: Vec<Crate>,
    pub mods: Vec<Mod>,
    /// `(file, reason)` for files that could not be read or parsed, or whose
    /// `mod` declarations point nowhere.
    pub problems: Vec<(String, String)>,
    /// Files claimed by several crate targets; only the first claim is kept.
    pub shared: Vec<String>,
    /// `(parent, name)` of a `mod name;` whose file another crate target
    /// already claimed, to the module that analyzes it.
    pub aliases: HashMap<(usize, String), usize>,
    by_abs: HashMap<PathBuf, usize>,
    root: PathBuf,
}

/// What a crate target is made of, before its modules are read.
pub struct Root {
    pub label: String,
    pub package: usize,
    pub kind: TargetKind,
    pub file: PathBuf,
    pub exported: bool,
    pub test_of: Option<String>,
}

enum Load {
    New(usize),
    Claimed,
    Unreadable,
}

struct Child {
    name: String,
    vis: Vis,
    cfg_test: bool,
    path: Option<String>,
    inline: Option<usize>,
}

impl Tree {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            ..Self::default()
        }
    }

    /// The items declared directly in a module.
    pub fn items(&self, m: usize) -> &[Item] {
        let module = &self.mods[m];
        let mut items = self.files[module.file].ast.items.as_slice();
        for &at in &module.inline {
            match &items[at] {
                Item::Mod(inner) => match &inner.content {
                    Some((_, nested)) => items = nested,
                    None => return &[],
                },
                _ => return &[],
            }
        }
        items
    }

    pub fn file_of(&self, abs: &Path) -> Option<usize> {
        self.by_abs.get(abs).copied()
    }

    pub fn add_crate(&mut self, root: Root) {
        let Load::New(file) = self.load(&root.file) else {
            return;
        };
        let krate = self.crates.len();
        let dir = root.file.parent().map(Path::to_owned).unwrap_or_default();
        let cfg_test = file_cfg_test(&self.files[file].ast);
        self.crates.push(Crate {
            package: root.package,
            kind: root.kind,
            exported: root.exported,
            root: self.mods.len(),
            test_of: root.test_of,
        });
        let at = self.mods.len();
        self.mods.push(Mod {
            name: root.label.clone(),
            path: root.label,
            parent: None,
            krate,
            file,
            inline: Vec::new(),
            vis: Vis::Pub,
            cfg_test,
            dir,
            nested: false,
        });
        self.children(at);
    }

    /// Parses a file once.
    fn load(&mut self, abs: &Path) -> Load {
        let abs = crate::provider::normalize(abs);
        if self.by_abs.contains_key(&abs) {
            return Load::Claimed;
        }
        let rel = abs
            .strip_prefix(&self.root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .replace('\\', "/");
        let text = match fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                self.problems.push((rel, format!("cannot read: {e}")));
                return Load::Unreadable;
            }
        };
        let ast = match syn::parse_file(&text) {
            Ok(ast) => ast,
            Err(e) => {
                let at = e.span().start();
                let (line, col) = (at.line, at.column + 1);
                self.problems
                    .push((rel.clone(), format!("{line}:{col}: {e}")));
                empty_file()
            }
        };
        let at = self.files.len();
        self.by_abs.insert(abs, at);
        self.files.push(SourceFile {
            generated: generated_header(&text),
            line_starts: line_starts(&text),
            rel,
            text,
            ast,
        });
        Load::New(at)
    }

    fn children(&mut self, parent: usize) {
        let declared = self.declared_children(parent);
        for child in declared {
            self.add_child(parent, child);
        }
    }

    fn declared_children(&self, parent: usize) -> Vec<Child> {
        self.items(parent)
            .iter()
            .enumerate()
            .filter_map(|(at, item)| {
                let Item::Mod(m) = item else {
                    return None;
                };
                Some(Child {
                    name: m.ident.to_string().trim_start_matches("r#").to_owned(),
                    vis: vis(&m.vis),
                    cfg_test: attrs_cfg_test(&m.attrs),
                    path: path_attribute(&m.attrs),
                    inline: m.content.as_ref().map(|_| at),
                })
            })
            .collect()
    }

    fn add_child(&mut self, parent: usize, child: Child) {
        let (path, krate, parent_file) = {
            let p = &self.mods[parent];
            (format!("{}/{}", p.path, child.name), p.krate, p.file)
        };
        let cfg_test = self.mods[parent].cfg_test || child.cfg_test;
        let base = self.mods[parent].dir.clone();
        let (file, inline, dir, nested) = match child.inline {
            Some(at) => {
                let mut inline = self.mods[parent].inline.clone();
                inline.push(at);
                let dir = base.join(child.path.as_deref().unwrap_or(&child.name));
                (parent_file, inline, dir, true)
            }
            None => {
                let Some((file, dir)) = self.file_module(parent, &child, &base) else {
                    return;
                };
                (file, Vec::new(), dir, false)
            }
        };
        let at = self.mods.len();
        self.mods.push(Mod {
            path,
            name: child.name,
            parent: Some(parent),
            krate,
            file,
            inline,
            vis: child.vis,
            cfg_test,
            dir,
            nested,
        });
        self.children(at);
    }

    /// Finds, loads and returns the file of `mod name;` with the directory its
    /// own children are searched in.
    fn file_module(
        &mut self,
        parent: usize,
        child: &Child,
        base: &Path,
    ) -> Option<(usize, PathBuf)> {
        let candidates: Vec<(PathBuf, bool)> = match &child.path {
            Some(path) => {
                let from = if self.mods[parent].nested {
                    base.to_owned()
                } else {
                    self.parent_dir(parent)
                };
                vec![(from.join(path), true)]
            }
            None => vec![
                (base.join(format!("{}.rs", child.name)), false),
                (base.join(&child.name).join("mod.rs"), true),
            ],
        };
        let existing = candidates.iter().find(|(p, _)| p.is_file()).cloned();
        let Some((found, mod_rs)) =
            existing.map(|(p, kind)| (crate::provider::normalize(&p), kind))
        else {
            let rel = self.files[self.mods[parent].file].rel.clone();
            self.problems.push((
                rel,
                format!(
                    "`mod {};` has no file: expected {}",
                    child.name,
                    describe(&candidates, &self.root)
                ),
            ));
            return None;
        };
        let file = match self.load(&found) {
            Load::New(file) => file,
            Load::Unreadable => return None,
            Load::Claimed => {
                self.alias(parent, &child.name, &found);
                let rel = found
                    .strip_prefix(&self.root)
                    .unwrap_or(&found)
                    .to_string_lossy()
                    .replace('\\', "/");
                self.shared.push(rel);
                return None;
            }
        };
        let dir = if mod_rs {
            found.parent().map(Path::to_owned).unwrap_or_default()
        } else {
            found.with_extension("")
        };
        Some((file, dir))
    }

    fn alias(&mut self, parent: usize, name: &str, file: &Path) {
        let Some(&claimed) = self.by_abs.get(file) else {
            return;
        };
        let owner = self
            .mods
            .iter()
            .position(|m| m.file == claimed && m.inline.is_empty());
        if let Some(owner) = owner {
            self.aliases.insert((parent, name.to_owned()), owner);
        }
    }

    /// Directory of the file a module is declared in.
    fn parent_dir(&self, m: usize) -> PathBuf {
        let rel = &self.files[self.mods[m].file].rel;
        let abs = self.root.join(rel);
        abs.parent().map(Path::to_owned).unwrap_or_default()
    }
}

fn describe(candidates: &[(PathBuf, bool)], root: &Path) -> String {
    let names: Vec<String> = candidates
        .iter()
        .map(|(p, _)| {
            p.strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.join(" or ")
}

fn empty_file() -> syn::File {
    syn::File {
        shebang: None,
        attrs: Vec::new(),
        items: Vec::new(),
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
    starts
}

/// `@generated` or `DO NOT EDIT` in the comments before the first item.
fn generated_header(text: &str) -> bool {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("//") && !line.starts_with("/*") && !line.starts_with('*') {
            return false;
        }
        if line.contains("@generated") || line.contains("DO NOT EDIT") {
            return true;
        }
    }
    false
}

/// `#[path = "x.rs"]`, or the first `path` inside `#[cfg_attr(cond, path = ..)]`:
/// only one file can stand for the module, the other alternatives of a
/// conditional path are never reached.
fn path_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| match &a.meta {
        Meta::NameValue(nv) => path_value(nv),
        Meta::List(list) if list.path.is_ident("cfg_attr") => {
            let parsed = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .ok()?;
            parsed.iter().skip(1).find_map(|meta| match meta {
                Meta::NameValue(nv) => path_value(nv),
                _ => None,
            })
        }
        _ => None,
    })
}

fn path_value(nv: &syn::MetaNameValue) -> Option<String> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = &nv.value
    else {
        return None;
    };
    nv.path.is_ident("path").then(|| s.value())
}

fn file_cfg_test(file: &syn::File) -> bool {
    attrs_cfg_test(&file.attrs)
}

/// Whether an attribute list gates the item on `cfg(test)`.
pub fn attrs_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && matches!(&a.meta, Meta::List(list) if mentions_test(list.tokens.clone()))
    })
}

fn mentions_test(tokens: TokenStream) -> bool {
    let mut previous_not = false;
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) if ident == "test" => return true,
            TokenTree::Ident(ident) => previous_not = ident == "not",
            TokenTree::Group(group) => {
                if !previous_not && mentions_test(group.stream()) {
                    return true;
                }
                previous_not = false;
            }
            _ => previous_not = false,
        }
    }
    false
}
