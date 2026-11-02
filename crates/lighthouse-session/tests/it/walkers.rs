//! Every command that lists the files of a project lists the same ones.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use lighthouse_session::{CheckRequest, baseline};

use crate::fixture::{LONG, canonical, project, session, write};

#[test]
fn the_check_validation_the_baseline_and_the_copy_list_the_same_files() {
    let dir = project("error");
    let root = canonical(dir.path());
    for file in [
        "b.txt",
        "docs/c.txt",
        ".hidden/d.txt",
        "out/e.txt",
        "third/f.txt",
        ".lighthouse/cache/g.txt",
        ".git/h.txt",
        "m.yaml",
        "out/n.yaml",
        ".hidden/o.yaml",
        "cfg/p.txt",
        "cfg/q.yaml",
    ] {
        write(&root, file, LONG);
    }
    write(&root, ".gitignore", "out/\n");
    write(
        &root,
        "lighthouse.toml",
        &lighthouse_test_support::project(
            "ignores = [\"cfg/\"]\n[rules]\n\"local/short\" = \"error\"\n",
        ),
    );
    write(&root, ".gitattributes", "third/** linguist-vendored\n");
    let session = session(&root);
    let listing = session.project_files(&root).unwrap().list();
    assert!(!listing.files.contains(&PathBuf::from("cfg/p.txt")));
    let listed: BTreeSet<PathBuf> = listing.files.iter().cloned().collect();

    // The check reports on every listed text file, and on no other.
    let checked: BTreeSet<PathBuf> = lighthouse_session::check(
        &session,
        &CheckRequest {
            paths: vec![session.root.clone()],
            ..CheckRequest::default()
        },
    )
    .unwrap()
    .findings()
    .into_iter()
    .map(|d| d.file)
    .collect();
    let texts: BTreeSet<PathBuf> = listed
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "txt"))
        .cloned()
        .collect();
    assert_eq!(checked, texts);

    // The baseline of an edit exists for every listed file and for none other.
    for path in [
        "b.txt",
        "docs/c.txt",
        ".hidden/d.txt",
        "out/e.txt",
        "third/f.txt",
        ".lighthouse/cache/g.txt",
        ".git/h.txt",
        "cfg/p.txt",
    ] {
        let path = PathBuf::from(path);
        let editable = baseline::editable(&root.join(&path), &session)
            .unwrap()
            .is_some();
        // Vendored files are decided by `.gitattributes`, which a path alone does not carry.
        if path != Path::new("third/f.txt") {
            assert_eq!(editable, listed.contains(&path), "{}", path.display());
        }
    }

    // Validation reads the documents of the listed files only.
    let validated =
        lighthouse_session::validate_paths(&session, std::slice::from_ref(&root), false).unwrap();
    let documents = listed
        .iter()
        .filter(|f| lighthouse_model::Format::of_path(f).is_some())
        .count();
    assert_eq!(validated.files, documents);

    // The copy holds the listed files, less the secrets.
    let into = tempfile::tempdir().unwrap();
    let placed =
        lighthouse_process::copy_project(&root, &listing.files, into.path(), &Default::default())
            .unwrap();
    assert_eq!(placed, listed);
}
