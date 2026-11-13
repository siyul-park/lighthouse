//! The scratch project of one cell and language: every (shape, arrangement)
//! as its own package in one directory, so one engine run reads them all.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use tempfile::TempDir;

use super::{Alphabet, Arrangement, Language, Shape, text};

const GO_MODULE: &str = "module scratch\n\ngo 1.22\n";
const RUST_MANIFEST: &str = "[package]\nname = \"scratch\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n";

/// A temporary project that holds the rendered packages. The directory is
/// removed when it is dropped.
#[derive(Debug)]
pub struct Scratch {
    dir: TempDir,
    units: Vec<Unit>,
}

/// One arrangement of one shape as a package of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The index of the shape in the list the project was rendered from.
    pub shape: usize,
    pub arrangement: Arrangement,
    /// The package name, `p0`, `p1`, ...
    pub package: String,
    /// The files, relative to the project root.
    pub files: Vec<PathBuf>,
}

impl Scratch {
    /// Renders every arrangement of every shape as a package of a new
    /// project in `language`; `alphabet` names the hook.
    pub fn render(language: Language, alphabet: Alphabet, shapes: &[Shape]) -> io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("lighthouse-witness-")
            .tempdir()?;
        let mut units = Vec::new();
        for (shape_index, shape) in shapes.iter().enumerate() {
            for arrangement in shape.arrangements() {
                let package = format!("p{}", units.len());
                let rendered = text::files(language, alphabet, shape, &arrangement, &package);
                let files = write(language, dir.path(), &package, &rendered)?;
                units.push(Unit {
                    shape: shape_index,
                    arrangement,
                    package,
                    files,
                });
            }
        }
        write_root(language, dir.path(), &units)?;
        Ok(Self { dir, units })
    }

    /// The directory of the project.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    /// The packages, in the order they were rendered.
    pub fn units(&self) -> &[Unit] {
        &self.units
    }
}

/// Writes the files of one package: Go in a directory of its own, Rust as a
/// module of the crate.
fn write(
    language: Language,
    root: &Path,
    package: &str,
    files: &[text::File],
) -> io::Result<Vec<PathBuf>> {
    let folder = match language {
        Language::Go => PathBuf::from(package),
        Language::Rust => PathBuf::from("src"),
    };
    fs::create_dir_all(root.join(&folder))?;
    let mut paths = Vec::new();
    for (name, body) in files {
        let path = folder.join(name);
        fs::write(root.join(&path), body)?;
        paths.push(path);
    }
    Ok(paths)
}

/// The files that make the packages a project: the Go module, or the crate
/// manifest and the root that declares one module per package.
fn write_root(language: Language, root: &Path, units: &[Unit]) -> io::Result<()> {
    match language {
        Language::Go => fs::write(root.join("go.mod"), GO_MODULE),
        Language::Rust => {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("Cargo.toml"), RUST_MANIFEST)?;
            let modules: String = units
                .iter()
                .map(|u| format!("mod {};\n", u.package))
                .collect();
            fs::write(root.join("src/lib.rs"), modules)
        }
    }
}
