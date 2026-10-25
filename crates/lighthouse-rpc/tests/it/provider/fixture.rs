//! A Go project on disk, the provider through the adapter, and the shorthands
//! the tests state their expectations with.

use std::{fs, path::Path, time::Duration};

use lighthouse_model::ucm::{
    EdgeKind, File, Flow, FlowKind, Fragment, FunctionSummary, Node, Target,
};
use lighthouse_plugin::{Indexed, LanguageProvider, Plugin, Source, Workspace};
use lighthouse_rpc::RpcPlugin;
use tempfile::TempDir;

pub(super) struct Fixture {
    pub(super) dir: TempDir,
    pub(super) provider: Box<dyn LanguageProvider>,
}

impl Fixture {
    /// `None` after a skip message when no Go toolchain is available.
    pub(super) fn new(files: &[(&str, &str)]) -> Option<Self> {
        let plugin = lighthouse_test_support::lang_go()?;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::write(root.join("go.mod"), "module example.com/app\n\ngo 1.26\n").unwrap();
        for (path, text) in files {
            write(&root, path, text);
        }
        let found = lighthouse_rpc::load(&plugin).unwrap();
        let rpc = RpcPlugin::connect(&found, &root, Duration::from_secs(120)).unwrap();
        let provider = rpc.languages().remove(0);
        Some(Self { dir, provider })
    }

    pub(super) fn root(&self) -> std::path::PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    pub(super) fn add(&self, path: &str, text: &str) {
        write(&self.root(), path, text);
    }

    pub(super) fn index(&self) -> Indexed {
        let root = self.root();
        let mut paths = Vec::new();
        collect(&root, &root, &mut paths);
        paths.sort();
        let files: Vec<File> = paths
            .iter()
            .map(|p| File {
                path: p.into(),
                lang: "go".to_owned(),
                hash: String::new(),
                generated: false,
                test: p.ends_with("_test.go"),
                facts: Default::default(),
            })
            .collect();
        let texts: Vec<String> = paths
            .iter()
            .map(|p| fs::read_to_string(root.join(p)).unwrap())
            .collect();
        let sources: Vec<Source> = files
            .iter()
            .zip(&texts)
            .map(|(file, text)| Source {
                file,
                text,
                regions: &[],
            })
            .collect();
        self.provider
            .index(&Workspace::new(&root), &sources)
            .unwrap()
    }

    /// The indexed fragment of one file.
    pub(super) fn fragment(&self, indexed: &Indexed, path: &str) -> Fragment {
        let want = Path::new(path);
        indexed
            .fragments
            .iter()
            .find(|f| f.files[0].path == want)
            .unwrap_or_else(|| panic!("no fragment for {path}"))
            .clone()
    }
}

pub(super) fn write(root: &Path, path: &str, text: &str) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, text).unwrap();
}

pub(super) fn collect(dir: &Path, root: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect(&path, root, out);
        } else if path.extension().is_some_and(|e| e == "go") {
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
}

pub(super) fn target(to: &Target) -> String {
    match to {
        Target::Path(p) => p.clone(),
        Target::Resolved(Node::Symbol(id)) => id.as_str().to_owned(),
        Target::Resolved(Node::Module(m)) => m.clone(),
    }
}

/// Targets of edges of `kind` leaving the symbol or module whose id contains `from`.
pub(super) fn edges(fragment: &Fragment, kind: EdgeKind, from: &str) -> Vec<String> {
    fragment
        .edges
        .iter()
        .filter(|e| e.kind == kind)
        .filter(|e| match &e.from {
            Node::Symbol(id) => id.as_str().contains(from),
            Node::Module(m) => m == from,
        })
        .map(|e| target(&e.to))
        .collect()
}

pub(super) fn summary<'f>(fragment: &'f Fragment, name: &str) -> &'f FunctionSummary {
    fragment
        .functions
        .iter()
        .find(|f| f.symbol.as_str().contains(name))
        .unwrap_or_else(|| panic!("no summary for {name}"))
}

pub(super) fn flow(kind: FlowKind, nesting: u32) -> Flow {
    Flow::new(kind, nesting)
}

pub(super) fn switch(arms: u32, returning: bool) -> Flow {
    Flow {
        arms,
        returning,
        ..Flow::new(FlowKind::Switch, 0)
    }
}

pub(super) fn logic(operators: u32) -> Flow {
    Flow {
        operators,
        ..Flow::new(FlowKind::Logic, 0)
    }
}

pub(super) const STORE: &str = r#"package store

import (
	"fmt"

	util "example.com/app/internal/util"
)

// Store keeps values.
type Store struct {
	items map[string]int
	Name  string
}

type Reader interface{ Read() }

const Limit = 3

var cache = map[string]int{}

// Get returns a value.
func (s *Store) Get(k string) int {
	if k == "" {
		return util.Zero()
	}
	fmt.Println(helper(k))
	return s.items[k]
}

func helper(k string) int { return len(k) }

func New() *Store { return &Store{items: map[string]int{}} }

func init() {}
"#;

pub(super) const UTIL: &str = "package util\n\n// Zero is zero.\nfunc Zero() int { return 0 }\n";

pub(super) fn store() -> [(&'static str, &'static str); 2] {
    [
        ("internal/store/store.go", STORE),
        ("internal/util/util.go", UTIL),
    ]
}
