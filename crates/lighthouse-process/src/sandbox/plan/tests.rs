use super::*;

fn paths(texts: &[&str]) -> Vec<PathBuf> {
    texts.iter().map(PathBuf::from).collect()
}

#[test]
fn a_directory_below_a_mask_is_read_again_unless_it_is_written() {
    let mask = paths(&["/run", "/tmp"]);

    assert_eq!(
        exposed(Path::new("/tmp/p"), &paths(&["/tmp/s"]), &mask),
        paths(&["/tmp/p"])
    );
    assert!(exposed(Path::new("/tmp/p/src"), &paths(&["/tmp/p"]), &mask).is_empty());
    assert!(exposed(Path::new("/home/u/p"), &[], &mask).is_empty());
}
