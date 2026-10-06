use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use lighthouse_model::{Edge, EdgeKind, Node, Resolution, Target};
use lighthouse_syntax::{Match, text};

/// Import alias to module path (internal packages) or import path.
pub(crate) type Aliases = BTreeMap<String, String>;

pub(crate) struct Imports {
    pub aliases: Aliases,
    pub edges: Vec<Edge>,
}

struct GoMod {
    dir: String,
    name: String,
}

/// Maps import paths onto workspace module paths through the nearest `go.mod`.
#[derive(Default)]
pub(crate) struct Resolver {
    cache: Mutex<BTreeMap<String, Option<Arc<GoMod>>>>,
}

impl Resolver {
    pub(crate) fn resolve(
        &self,
        root: &Path,
        dir: &str,
        module: &str,
        specs: &[Match],
        source: &str,
    ) -> Imports {
        let gomod = self.nearest(root, dir);
        let mut aliases = Aliases::new();
        let mut edges = Vec::new();
        for spec in specs {
            let Some(path) = spec.get("path").map(|n| unquote(text(n, source))) else {
                continue;
            };
            if path == "C" {
                continue;
            }
            let target = gomod.as_deref().map_or(path.clone(), |m| m.locate(&path));
            let alias = spec.get("alias").map(|n| text(n, source).to_owned());
            match alias.as_deref() {
                Some("_" | ".") => {}
                Some(name) => {
                    aliases.insert(name.to_owned(), target.clone());
                }
                None => {
                    aliases.insert(default_alias(&path), target.clone());
                }
            }
            edges.push(Edge {
                kind: EdgeKind::Imports,
                from: Node::Module(module.to_owned()),
                to: Target::Path(target),
                resolution: Resolution::Syntactic,
            });
        }
        Imports { aliases, edges }
    }

    fn nearest(&self, root: &Path, dir: &str) -> Option<Arc<GoMod>> {
        let mut cache = self.cache.lock().ok()?;
        let mut at = if dir == "." { "" } else { dir };
        let mut visited = Vec::new();
        let found = loop {
            if let Some(hit) = cache.get(at) {
                break hit.clone();
            }
            visited.push(at.to_owned());
            if let Some(name) = module_name(&root.join(at).join("go.mod")) {
                break Some(Arc::new(GoMod {
                    dir: at.to_owned(),
                    name,
                }));
            }
            if at.is_empty() {
                break None;
            }
            at = at.rsplit_once('/').map_or("", |(parent, _)| parent);
        };
        for key in visited {
            cache.insert(key, found.clone());
        }
        found
    }
}

impl GoMod {
    fn locate(&self, import: &str) -> String {
        let rest = if import == self.name {
            ""
        } else if let Some(rest) = import
            .strip_prefix(&self.name)
            .and_then(|r| r.strip_prefix('/'))
        {
            rest
        } else {
            return import.to_owned();
        };
        match (self.dir.as_str(), rest) {
            ("", "") => ".".to_owned(),
            ("", rest) => rest.to_owned(),
            (dir, "") => dir.to_owned(),
            (dir, rest) => format!("{dir}/{rest}"),
        }
    }
}

fn module_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let name = line.trim().strip_prefix("module")?;
        let name = name.trim().trim_matches('"');
        (!name.is_empty()).then(|| name.to_owned())
    })
}

fn default_alias(path: &str) -> String {
    let mut parts = path.rsplit('/');
    let last = parts.next().unwrap_or(path);
    let is_version =
        |s: &str| s.len() > 1 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit());
    let name = match parts.next() {
        Some(previous) if is_version(last) => previous,
        _ => last,
    };
    name.split('.').next().unwrap_or(name).to_owned()
}

fn unquote(literal: &str) -> String {
    literal.trim_matches(['"', '`']).to_owned()
}
